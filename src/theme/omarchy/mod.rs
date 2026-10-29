//! 「跟随 Omarchy」主题：读 Omarchy 当前主题的 `colors.toml`，换算成界面配色。
//!
//! Omarchy 没有单一配色，它有二十几套内置主题外加用户自装的主题，切主题时
//! 终端、Waybar、Hyprland 边框一起换色。这里不做固定预设，而是读当前主题的
//! 真源 `colors.toml` 现算一套 [`Theme`]。只读，不往用户的 Omarchy 配置里写任何东西。
//!
//! 启动与点选主题卡片时各读一次（[`refresh`]），之后由后台线程每秒看一眼
//! （[`start_watcher`]），Omarchy 换了主题就重读；换算结果缓存在本模块，每成功换算
//! 一次 [`generation`] 加一，界面线程据此重套样式。方案与分期见
//! `docs/omarchy-theme-design.md`。

mod colors;
mod derive;
mod detect;
mod watch;

use crate::theme::Theme;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) use watch::start as start_watcher;

/// 设置页与菜单里的主题名。
pub(crate) const LABEL: &str = "跟随 Omarchy";

struct Loaded {
    theme: Theme,
    /// Omarchy 当前主题的显示名，例如「Tokyo Night」。
    name: Option<String>,
}

/// 最近一次成功换算的结果。读失败时保留上一份，不清空。
static LOADED: RwLock<Option<Loaded>> = RwLock::new(None);

/// 缓存的版本号，每成功换算一次加一。
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 重新读 Omarchy 当前主题并换算。找不到目录或解析失败时返回原因，
/// 已缓存的上一份配色保持不变。
pub(crate) fn refresh() -> Result<(), String> {
    let dir = detect::theme_dir().ok_or_else(|| "没有找到 Omarchy 当前主题".to_string())?;
    let colors = colors::read(&dir.join(detect::COLORS_FILE))?;
    let loaded = Loaded {
        theme: derive::derive(&colors),
        name: detect::theme_name(&dir),
    };
    *LOADED
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(loaded);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// 缓存的版本号。界面线程记下自己套用过的那一版，变了就重套样式。
pub(crate) fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// 是否读到过一份可用的 Omarchy 配色；没有时界面上不出现「跟随 Omarchy」。
pub(crate) fn available() -> bool {
    LOADED
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some()
}

/// 换算好的界面配色；从未读到过时为 `None`。
pub(crate) fn palette() -> Option<Theme> {
    LOADED
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|loaded| loaded.theme)
}

/// Omarchy 当前主题的显示名。
pub(crate) fn theme_name() -> Option<String> {
    LOADED
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .and_then(|loaded| loaded.name.clone())
}

#[cfg(test)]
mod tests {
    use super::derive::{
        MUTED_MIN, ON_ACCENT_MIN, SOFT_MIN, TEXT_MIN, contrast, delta_e, luminance,
    };
    use super::*;
    use eframe::egui::Color32;
    use std::path::PathBuf;

    /// Omarchy 内置主题的 `colors.toml` 样本（MIT 许可，见 THIRD_PARTY_NOTICES.md）。
    fn fixtures() -> Vec<(String, colors::Colors)> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/omarchy");
        let mut themes: Vec<_> = std::fs::read_dir(&dir)
            .expect("样本目录应可读")
            .map(|entry| entry.expect("目录项").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .map(|path| {
                let name: String = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into();
                let colors = colors::read(&path).unwrap_or_else(|error| panic!("{error}"));
                (name, colors)
            })
            .collect();
        themes.sort_by(|a, b| a.0.cmp(&b.0));
        themes
    }

    fn fixture(name: &str) -> (colors::Colors, Theme) {
        let (_, colors) = fixtures()
            .into_iter()
            .find(|(theme, _)| theme == name)
            .unwrap_or_else(|| panic!("缺少样本 {name}"));
        let theme = derive::derive(&colors);
        (colors, theme)
    }

    #[test]
    fn all_builtin_themes_are_sampled() {
        assert_eq!(fixtures().len(), 23, "Omarchy 内置主题样本数量变了");
    }

    /// 每一套内置主题换算出来都要读得清：三档文字、强调色、按钮字、语义色、
    /// Markdown 高亮各按自己的标准过对比度。
    #[test]
    fn every_builtin_theme_stays_readable() {
        let mut failures = Vec::new();
        for (name, colors) in fixtures() {
            let theme = derive::derive(&colors);
            let mut check = |what: &str, fg: Color32, bg: Color32, min: f32| {
                let ratio = contrast(fg, bg);
                if ratio < min {
                    failures.push(format!("{name}：{what} 对比度 {ratio:.2} < {min}"));
                }
            };
            for (ground, label) in [
                (theme.surface, "卡片"),
                (theme.canvas, "画布"),
                (theme.surface_sunk, "沉底"),
            ] {
                check(&format!("正文/{label}"), theme.text, ground, TEXT_MIN);
                check(&format!("次级/{label}"), theme.text_soft, ground, SOFT_MIN);
                check(
                    &format!("弱化/{label}"),
                    theme.text_muted,
                    ground,
                    MUTED_MIN,
                );
            }
            check("强调/画布", theme.accent, theme.canvas, MUTED_MIN);
            check("强调/卡片", theme.accent, theme.surface, MUTED_MIN);
            check("强调/淡底", theme.accent, theme.accent_soft, MUTED_MIN);
            for (state, ground) in [
                ("常态", theme.accent),
                ("悬停", theme.accent_hover),
                ("按下", theme.accent_active),
            ] {
                check(
                    &format!("按钮字/{state}"),
                    theme.on_accent,
                    ground,
                    ON_ACCENT_MIN,
                );
            }
            for (what, color) in [
                ("警告", theme.warn),
                ("危险", theme.danger),
                ("成功", theme.success),
                ("提示", theme.info),
            ] {
                check(what, color, theme.surface, MUTED_MIN);
            }
            let md = theme.md;
            for (what, color) in [
                ("md 标题", md.title),
                ("md 小标题", md.heading),
                ("md 加粗", md.strong),
                ("md 表格", md.table_cell),
                ("md 代码", md.code),
                ("md 引号", md.quoted),
                ("md 待核实", md.todo),
            ] {
                check(what, color, theme.surface, SOFT_MIN);
            }
            check("md 正文/搜索底", md.body, md.search_bg, SOFT_MIN);
            check("md 正文/锚点底", md.body, md.anchor_bg, SOFT_MIN);
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// 画布、卡片、悬停、按下几层底色要分得出来，明暗标志要与画布一致。
    #[test]
    fn every_builtin_theme_keeps_its_layers_apart() {
        let mut failures = Vec::new();
        for (name, colors) in fixtures() {
            let theme = derive::derive(&colors);
            for (what, a, b) in [
                ("画布/卡片", theme.canvas, theme.surface),
                ("沉底/卡片", theme.surface_sunk, theme.surface),
                ("悬停/卡片", theme.surface_hover, theme.surface),
                ("按下/卡片", theme.surface_active, theme.surface),
                ("描边/卡片", theme.border, theme.surface),
                ("强描边/描边", theme.border_strong, theme.border),
            ] {
                let distance = delta_e(a, b);
                if distance < 2.0 {
                    failures.push(format!("{name}：{what} 只差 ΔE {distance:.1}"));
                }
            }
            if theme.dark != (luminance(theme.canvas) < 0.18) {
                failures.push(format!("{name}：dark 标志与画布明暗不符"));
            }
            if colors.dark != Some(theme.dark) {
                failures.push(format!("{name}：没有照 mode 定明暗"));
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// 卡片用主题原本的 background，用户一眼认得出这是哪套主题。没有更暗的
    /// 底可当画布时（近黑主题），background 改当画布，卡片提亮一层。
    #[test]
    fn surface_keeps_the_theme_background() {
        for (name, colors) in fixtures() {
            let theme = derive::derive(&colors);
            let background = colors.get("background");
            if Some(theme.canvas) == background {
                assert!(theme.dark, "{name}：只有深色主题会提亮卡片");
                continue;
            }
            assert_eq!(Some(theme.surface), background, "{name}");
        }
    }

    /// rose-pine 的 lighter_background 反而比 background 暗：按亮度排，画布照样
    /// 比卡片深。
    #[test]
    fn rose_pine_layers_follow_brightness_not_key_names() {
        let (colors, theme) = fixture("rose-pine");
        assert!(!theme.dark);
        assert!(luminance(theme.canvas) < luminance(theme.surface));
        assert_eq!(Some(theme.canvas), colors.get("lighter_background"));
        assert_eq!(theme.paper_family, crate::theme::PaperFamily::Parchment);
    }

    /// tokyo-night 的 dark_foreground 压在底色上不到 3:1，弱化文字要被推亮。
    #[test]
    fn tokyo_night_muted_text_is_lifted() {
        let (colors, theme) = fixture("tokyo-night");
        let raw = colors
            .get("dark_foreground")
            .expect("样本有 dark_foreground");
        assert!(contrast(raw, theme.surface) < MUTED_MIN);
        assert_ne!(theme.text_muted, raw);
        assert!(luminance(theme.text_muted) > luminance(raw));
        assert_eq!(theme.paper_family, crate::theme::PaperFamily::NightBlue);
    }

    /// 外壳形状靠 label 认出「跟随 Omarchy」（`theme::square_chrome`），每套都得带上它。
    #[test]
    fn every_builtin_theme_carries_the_omarchy_label() {
        for (name, colors) in fixtures() {
            assert_eq!(derive::derive(&colors).label, LABEL, "{name}");
        }
    }

    /// 纯黑主题没有更暗的底，卡片往前景提一点，与画布分开。
    #[test]
    fn vantablack_lifts_the_surface_off_the_black_canvas() {
        let (colors, theme) = fixture("vantablack");
        assert_eq!(Some(theme.canvas), colors.get("background"));
        assert!(luminance(theme.surface) > luminance(theme.canvas));
    }

    /// white 的强调色是中灰：够得着 3:1 就原样保留，不另找色相。
    #[test]
    fn white_keeps_its_grey_accent() {
        let (colors, theme) = fixture("white");
        assert_eq!(Some(theme.accent), colors.get("accent"));
    }

    /// matte-black 的强调色是亮橙，白字压不住，按钮字要换成深色。
    #[test]
    fn matte_black_buttons_use_dark_text() {
        let (_, theme) = fixture("matte-black");
        assert!(luminance(theme.on_accent) < 0.2);
    }
}
