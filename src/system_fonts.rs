//! 本机字体枚举。
//!
//! 编译公文时可以用本机安装的字体替换随应用分发的内置字体。这里负责把字体目录
//! 里的字体列出来，并从 `name` 表读出家族名：家族名给导出的 `.tex` 按名字加载用，
//! 文件路径给内置 Tectonic 按文件加载用，两者都要存进配置。
//!
//! 只收录 `.ttf` 与 `.otf`。字体集合（`.ttc` / `.otc`）一个文件里装着多个字面，
//! 按文件加载时必须额外指定字面序号，这条路在内置 Tectonic 上没有验证过，因此
//! 直接排除，不让用户选到编译不出来的字体。

use crate::models::{FontChoice, FontConfig, FontRole};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 可以选来编译的字体文件扩展名，与文件选择对话框的过滤器共用。
pub const SUPPORTED_EXTENSIONS: &[&str] = &["ttf", "otf"];

/// 目录递归深度上限。Linux 的字体目录按厂商分层，Windows 则是平铺的。
const MAX_DEPTH: usize = 4;

/// Windows `name` 表的语言 ID：用来在多语言字体名里挑出想要的那条。
const LANGUAGE_SIMPLIFIED_CHINESE: u16 = 0x0804;
const LANGUAGE_ENGLISH_US: u16 = 0x0409;

/// 一个可供选择的本机字体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemFont {
    /// 英文家族名（`name` 表 ID 1），fontspec 按名字加载时使用。
    pub family: String,
    /// 本地化家族名，只用于界面显示；没有中文名时与 `family` 相同。
    pub display: String,
    pub path: PathBuf,
}

impl SystemFont {
    pub fn to_choice(&self) -> FontChoice {
        FontChoice {
            family: self.family.clone(),
            display: self.display.clone(),
            path: self.path.display().to_string(),
        }
    }
}

/// 扫描本机字体目录。同一家族只保留一个字面，优先常规字重；耗时主要在读取
/// 字体文件上（中文字体动辄十几兆），调用方应放到后台线程。
pub fn scan() -> Vec<SystemFont> {
    // 家族名去重：同名时常规字重优先，否则保留先遇到的那个。
    let mut found: BTreeMap<String, (bool, SystemFont)> = BTreeMap::new();
    for dir in font_dirs() {
        collect_dir(&dir, 0, &mut found);
    }
    let mut fonts: Vec<SystemFont> = found.into_values().map(|(_, font)| font).collect();
    fonts.sort_by(|a, b| a.display.cmp(&b.display).then(a.family.cmp(&b.family)));
    fonts
}

/// 读取单个字体文件，供“浏览字体文件”手动指定使用。字体集合与解析不了的
/// 文件都返回 `None`。
pub fn read_font(path: &Path) -> Option<SystemFont> {
    if !has_supported_extension(path) {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    parse_font(&data, path).map(|(font, _)| font)
}

/// 按配置逐项检查字体文件是否还在，缺失的项退回内置字体并给出提示。
///
/// 配置可能来自另一台机器，也可能字体后来被卸载了。缺文件时直接编译会失败，
/// 而公文的字体本就有内置的一套兜底，降级比报错有用。
pub fn resolve(fonts: &FontConfig) -> (FontConfig, Vec<String>) {
    let mut resolved = fonts.clone();
    let mut warnings = Vec::new();
    if !fonts.use_system_fonts {
        return (resolved, warnings);
    }
    for role in FontRole::ALL {
        let choice = resolved.choice_mut(role);
        if !choice.is_set() {
            continue;
        }
        if !Path::new(choice.path.trim()).is_file() {
            warnings.push(format!(
                "{}“{}”的字体文件不存在（{}），本次编译改用内置字体。",
                role.label(),
                choice.label(),
                choice.path.trim()
            ));
            *choice = FontChoice::default();
        }
    }
    (resolved, warnings)
}

fn has_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| ext.eq_ignore_ascii_case(supported))
        })
}

fn collect_dir(dir: &Path, depth: usize, found: &mut BTreeMap<String, (bool, SystemFont)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dir(&path, depth + 1, found);
            continue;
        }
        if !has_supported_extension(&path) {
            continue;
        }
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let Some((font, regular)) = parse_font(&data, &path) else {
            continue;
        };
        let key = font.family.to_lowercase();
        match found.get(&key) {
            // 已经收了常规字重就不再替换，否则同一家族会被粗体、细体轮流覆盖。
            Some((true, _)) => {}
            Some((false, _)) if !regular => {}
            _ => {
                found.insert(key, (regular, font));
            }
        }
    }
}

/// 解析字体，返回字体信息与“是不是常规字面”。同一家族有多个文件时靠后者
/// 挑出适合正文的那个。字体文件很大，解析只做这一遍。
fn parse_font(data: &[u8], path: &Path) -> Option<(SystemFont, bool)> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let names: Vec<ttf_parser::name::Name<'_>> = face.names().into_iter().collect();
    let family = pick_name(
        &names,
        ttf_parser::name::name_id::FAMILY,
        LANGUAGE_ENGLISH_US,
    )?;
    let display = pick_name(
        &names,
        ttf_parser::name::name_id::FAMILY,
        LANGUAGE_SIMPLIFIED_CHINESE,
    )
    .unwrap_or_else(|| family.clone());
    let regular = face.is_regular() && !face.is_italic() && !face.is_oblique();
    Some((
        SystemFont {
            family,
            display,
            path: path.to_owned(),
        },
        regular,
    ))
}

/// 取指定名称项：先按偏好语言找，找不到就退回任意一条可解码的同名项。
fn pick_name(names: &[ttf_parser::name::Name<'_>], name_id: u16, language: u16) -> Option<String> {
    let matching = |wanted: Option<u16>| {
        names
            .iter()
            .filter(|name| name.name_id == name_id)
            .filter(|name| wanted.is_none_or(|language| name.language_id == language))
            .find_map(|name| name.to_string())
    };
    let value = matching(Some(language)).or_else(|| matching(None))?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 各平台的字体安装目录，含用户级目录（Windows 10 起支持免管理员装字体）。
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        if let Some(windir) = std::env::var_os("WINDIR") {
            dirs.push(PathBuf::from(windir).join("Fonts"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(
                PathBuf::from(local)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts"),
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library").join("Fonts"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&home).join(".fonts"));
            dirs.push(
                PathBuf::from(&home)
                    .join(".local")
                    .join("share")
                    .join("fonts"),
            );
        }
    }
    dirs.retain(|dir| dir.is_dir());
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ttf_and_otf_are_selectable() {
        assert!(has_supported_extension(Path::new(
            "C:/Windows/Fonts/simfang.ttf"
        )));
        assert!(has_supported_extension(Path::new("/usr/share/fonts/x.OTF")));
        // 字体集合排除在外：按文件加载需要额外指定字面序号。
        assert!(!has_supported_extension(Path::new(
            "C:/Windows/Fonts/simsun.ttc"
        )));
        assert!(!has_supported_extension(Path::new("a.otc")));
        assert!(!has_supported_extension(Path::new("a.fon")));
    }

    #[test]
    fn read_font_rejects_collections_without_reading_them() {
        // 文件不存在也应先被扩展名挡住，不产生读盘错误。
        assert!(read_font(Path::new("C:/Windows/Fonts/nonexistent.ttc")).is_none());
    }

    #[test]
    fn resolve_drops_missing_files_and_reports_them() {
        let mut fonts = FontConfig {
            use_system_fonts: true,
            ..FontConfig::default()
        };
        *fonts.choice_mut(FontRole::Body) = FontChoice {
            family: "FangSong".into(),
            display: "仿宋".into(),
            path: "Z:/does-not-exist/FangSong.ttf".into(),
        };
        let (resolved, warnings) = resolve(&fonts);
        assert!(!resolved.choice(FontRole::Body).is_set());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("正文字体"));
    }

    #[test]
    fn resolve_keeps_everything_when_switch_is_off() {
        let mut fonts = FontConfig::default();
        *fonts.choice_mut(FontRole::Body) = FontChoice {
            family: "FangSong".into(),
            display: "仿宋".into(),
            path: "Z:/does-not-exist/FangSong.ttf".into(),
        };
        let (resolved, warnings) = resolve(&fonts);
        assert!(warnings.is_empty());
        assert_eq!(resolved.choice(FontRole::Body).family, "FangSong");
    }

    /// 真机扫描一次，确认能列出字体且不会慢到离谱（界面上是后台线程，但超过
    /// 十几秒就该改成流式返回了）。没有字体目录的构建环境直接跳过。
    #[test]
    #[ignore = "依赖本机字体目录，耗时随已装字体数量变化"]
    fn scanning_local_fonts_finds_families_in_reasonable_time() {
        let started = std::time::Instant::now();
        let fonts = scan();
        let elapsed = started.elapsed();
        println!("扫描到 {} 个字体，用时 {elapsed:?}", fonts.len());
        if fonts.is_empty() {
            return;
        }
        assert!(fonts.iter().all(|font| !font.family.is_empty()));
        assert!(fonts.iter().all(|font| has_supported_extension(&font.path)));
        assert!(elapsed.as_secs() < 20, "扫描耗时 {elapsed:?} 过长");
    }

    #[test]
    fn bundled_fonts_parse_into_named_families() {
        let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("font")
            .join("SimSun.ttf");
        if !bundled.is_file() {
            return; // 精简检出没有字体资产，跳过。
        }
        let font = read_font(&bundled).expect("内置字体应能解析出家族名");
        assert!(!font.family.is_empty());
        assert_eq!(font.path, bundled);
    }
}
