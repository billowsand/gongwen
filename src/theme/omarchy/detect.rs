//! 找 Omarchy 当前主题目录。
//!
//! 依次认：
//! 1. 环境变量 `GONGWEN_OMARCHY_THEME_DIR`（开发调试用，各平台都认，便于在
//!    Windows / macOS 上拿官方 `colors.toml` 样本调色）；
//! 2. `$XDG_STATE_HOME/omarchy/current/theme`（新版 Omarchy，缺省 `~/.local/state`）；
//! 3. `~/.config/omarchy/current/theme`（旧版 Omarchy）。
//!
//! 目录里有 `colors.toml` 才算数。后两项只在 Linux 上找。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 开发调试用的主题目录覆盖。
pub(crate) const ENV_OVERRIDE: &str = "GONGWEN_OMARCHY_THEME_DIR";

/// 主题目录下的配色文件名。
pub(crate) const COLORS_FILE: &str = "colors.toml";

/// 当前机器上 Omarchy 当前主题目录；找不到返回 `None`。
pub(crate) fn theme_dir() -> Option<PathBuf> {
    let base = directories::BaseDirs::new();
    let (state, home) = if cfg!(target_os = "linux") {
        (
            base.as_ref()
                .and_then(|dirs| dirs.state_dir().map(Path::to_path_buf)),
            base.as_ref().map(|dirs| dirs.home_dir().to_path_buf()),
        )
    } else {
        (None, None)
    };
    candidates(std::env::var_os(ENV_OVERRIDE), state, home)
        .into_iter()
        .find(|dir| dir.join(COLORS_FILE).is_file())
}

/// 按优先级排好的候选目录，不看磁盘，便于测试。
fn candidates(
    override_dir: Option<OsString>,
    state_dir: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = override_dir.filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(state) = state_dir {
        dirs.push(state.join("omarchy").join("current").join("theme"));
    }
    if let Some(home) = home {
        dirs.push(
            home.join(".config")
                .join("omarchy")
                .join("current")
                .join("theme"),
        );
    }
    dirs
}

/// Omarchy 当前主题的显示名。`theme.name` 存的是 `tokyo-night` 这样的小写
/// 连字符写法，按 `omarchy-theme-current` 的办法转成 `Tokyo Night`。
pub(crate) fn theme_name(theme_dir: &Path) -> Option<String> {
    let file = theme_dir.parent()?.join("theme.name");
    let bytes = crate::file_limits::read_bytes(&file, 4 * 1024, "Omarchy 主题名").ok()?;
    let slug = String::from_utf8_lossy(&bytes);
    let name = display_name(slug.trim());
    (!name.is_empty()).then_some(name)
}

fn display_name(slug: &str) -> String {
    slug.split('-')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_comes_first_then_new_then_old_layout() {
        let dirs = candidates(
            Some(OsString::from("/tmp/theme")),
            Some(PathBuf::from("/home/u/.local/state")),
            Some(PathBuf::from("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/tmp/theme"),
                PathBuf::from("/home/u/.local/state/omarchy/current/theme"),
                PathBuf::from("/home/u/.config/omarchy/current/theme"),
            ]
        );
        assert!(candidates(Some(OsString::new()), None, None).is_empty());
    }

    #[test]
    fn theme_name_reads_the_slug_beside_the_theme_dir() {
        let root = tempfile::tempdir().expect("临时目录");
        let theme = root.path().join("theme");
        std::fs::create_dir(&theme).expect("建目录");
        assert_eq!(theme_name(&theme), None);
        std::fs::write(root.path().join("theme.name"), "tokyo-night\n").expect("写入");
        assert_eq!(theme_name(&theme).as_deref(), Some("Tokyo Night"));
        assert_eq!(display_name("retro-82"), "Retro 82");
    }
}
