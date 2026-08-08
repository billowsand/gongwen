//! 便携式 TeX 运行时定位。
//!
//! 发布包把 Tectonic、固定 bundle 与字体放在可执行文件旁的 `runtime` 目录；
//! 开发构建则额外检查仓库根目录，方便直接运行测试而不污染 PATH。

use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const TECTONIC_VERSION: &str = "0.17.0";
pub const BUNDLE_FILE_NAME: &str = "gongwen-texlive.ttb";
pub const FONT_FILES: &[&str] = &[
    "FangSong.ttf",
    "KaiTi.ttf",
    "SimHei.ttf",
    "SimSun.ttf",
    "XiaoBiaoSong.ttf",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableTexRuntime {
    pub root: PathBuf,
    pub tectonic: PathBuf,
    pub bundle: PathBuf,
    pub fonts: PathBuf,
}

impl PortableTexRuntime {
    fn candidate(root: PathBuf) -> Self {
        Self {
            tectonic: root.join("tectonic").join("tectonic.exe"),
            bundle: root.join("texbundle").join(BUNDLE_FILE_NAME),
            fonts: root.join("fonts"),
            root,
        }
    }

    fn validate(&self) -> Result<()> {
        if !self.tectonic.is_file() {
            bail!("便携式 Tectonic 不存在：{}", self.tectonic.display());
        }
        if !self.bundle.is_file() {
            bail!("离线 TeX bundle 不存在：{}", self.bundle.display());
        }
        validate_font_dir(&self.fonts)
    }
}

/// 找到完整的便携式 TeX 运行时。只要发现了 `tectonic.exe`，其余资源缺失就明确
/// 报错，避免静默回退到另一套不可复现的系统环境。
pub fn find_tex_runtime() -> Result<Option<PortableTexRuntime>> {
    for root in runtime_roots() {
        let runtime = PortableTexRuntime::candidate(root);
        if runtime.tectonic.is_file() {
            runtime.validate()?;
            return Ok(Some(runtime));
        }
    }
    Ok(None)
}

/// 公文预览只需要字体，因此即使 Tectonic 尚未准备好，也可以使用仓库 `font`
/// 目录中的开发资产。
pub fn find_font_dir() -> Option<PathBuf> {
    for root in runtime_roots() {
        let fonts = root.join("fonts");
        if validate_font_dir(&fonts).is_ok() {
            return Some(fonts);
        }
    }

    let development_fonts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("font");
    validate_font_dir(&development_fonts)
        .is_ok()
        .then_some(development_fonts)
}

pub fn validate_font_dir(fonts: &Path) -> Result<()> {
    for file in FONT_FILES {
        let path = fonts.join(file);
        if !path.is_file() {
            bail!("便携式字体不存在：{}", path.display());
        }
    }
    Ok(())
}

/// Tectonic 只把自动生成的格式文件写入缓存；TeX 宏包始终来自显式指定的本地
/// bundle。测试或受控部署可用环境变量把缓存重定向到其他位置。
pub fn tectonic_cache_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("GONGWEN_TECTONIC_CACHE_DIR") {
        return Ok(PathBuf::from(path));
    }

    let project = directories::ProjectDirs::from("cn", "Gongwen", "GongwenAssistant")
        .context("无法确定 Tectonic 用户缓存目录")?;
    Ok(project.cache_dir().join("tectonic"))
}

fn runtime_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = std::env::var_os("GONGWEN_RUNTIME_DIR") {
        roots.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        roots.push(parent.join("runtime"));
    }
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime"));

    let mut seen = HashSet::new();
    roots.retain(|path| seen.insert(path.clone()));
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_layout_uses_stable_release_names() {
        let runtime = PortableTexRuntime::candidate(PathBuf::from("runtime"));
        assert!(runtime.tectonic.ends_with("tectonic/tectonic.exe"));
        assert!(runtime.bundle.ends_with("texbundle/gongwen-texlive.ttb"));
        assert!(runtime.fonts.ends_with("fonts"));
    }

    #[test]
    fn font_dir_validation_checks_expected_filenames() {
        let dir = tempfile::tempdir().expect("temporary directory must be creatable");
        for file in FONT_FILES {
            std::fs::write(dir.path().join(file), b"font")
                .expect("font placeholder must be writable");
        }
        validate_font_dir(dir.path()).expect("complete font directory must validate");

        std::fs::remove_file(dir.path().join(FONT_FILES[0]))
            .expect("font placeholder must be removable");
        assert!(validate_font_dir(dir.path()).is_err());
    }
}
