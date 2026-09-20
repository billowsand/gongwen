//! 便携式 TeX 运行时定位。
//!
//! 发布包把 Tectonic、固定 bundle 与字体放在可执行文件旁的 `runtime` 目录；
//! 开发构建则额外检查仓库根目录，方便直接运行测试而不污染 PATH。

use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const TECTONIC_VERSION: &str = "0.17.0";
pub const BUNDLE_FILE_NAME: &str = "gongwen-texlive.ttb";
#[cfg(windows)]
pub const TECTONIC_BINARY: &str = "tectonic.exe";
#[cfg(not(windows))]
pub const TECTONIC_BINARY: &str = "tectonic";
#[cfg(windows)]
const PLATFORM_SUFFIX: &str = "win-x64";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const PLATFORM_SUFFIX: &str = "linux-arm64";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const PLATFORM_SUFFIX: &str = "linux-amd64";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const PLATFORM_SUFFIX: &str = "darwin-arm64";
#[cfg(not(any(
    windows,
    all(
        target_os = "linux",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ),
    all(target_os = "macos", target_arch = "aarch64"),
)))]
compile_error!(
    "便携式 runtime 只发布 win-x64 / linux-amd64 / linux-arm64 / darwin-arm64 四个目标；\
     新增目标时请同时补上 PLATFORM_SUFFIX 和对应的 SHA256SUMS 清单。"
);

/// 公文文种排版所需的字体，由 `gonghan-gwa.cls` 按文件名加载。
///
/// 这一组是**全部** PDF 编译和纸面预览的下限：缺任何一个都排不出公文，因此
/// `find_tex_runtime` 与 `find_font_dir` 都按它校验。
pub const OFFICIAL_FONT_FILES: &[&str] = &[
    "FangSong.ttf",
    "KaiTi.ttf",
    "SimHei.ttf",
    "SimSun.ttf",
    "XiaoBiaoSong.ttf",
];

/// 研究报告排版所需的字体，由 mdx 的 `md2tex.cls` 在 `\MdxFontPath` 分支下按
/// 文件名加载——**文件名是与 md2tex.cls 的协议，改名要两边一起改**。
///
/// 单独成组是因为它只挡研究报告：老 runtime 目录缺这几个字体时，公文照常
/// 编译和预览，只有研究报告报错，不至于让整台机器失去出 PDF 的能力。
pub const RESEARCH_FONT_FILES: &[&str] = &[
    "FZShuSong.ttf",
    "FZHei.ttf",
    "FZKai.ttf",
    "FZXiaoBiaoSong.ttf",
    "JetBrainsMono-Regular.ttf",
    "texgyretermes-regular.otf",
    "texgyretermes-bold.otf",
    "texgyretermes-italic.otf",
    "texgyretermes-bolditalic.otf",
];

/// 两组字体的并集，发布包必须齐备。编译工作区一次性把它们全部链进临时字体
/// 目录：链接近乎零成本，省得按文种分两套目录。
pub fn font_files() -> impl Iterator<Item = &'static str> {
    OFFICIAL_FONT_FILES
        .iter()
        .chain(RESEARCH_FONT_FILES)
        .copied()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableTexRuntime {
    pub root: PathBuf,
    pub tectonic: PathBuf,
    pub bundle: PathBuf,
    pub fonts: PathBuf,
}

impl PortableTexRuntime {
    fn candidate(root: PathBuf) -> Self {
        // 发布包把本平台的二进制装成 `tectonic/tectonic` 并带上执行位；开发仓库
        // 与刚解压的 runtime 压缩包里则是按平台分目录的 `tectonic/<平台>/tectonic`。
        //
        // 两处都在时必须挑**真正可执行**的那一个：zip 不保存 Unix 权限位，解压
        // 出来的那份往往没有执行位，选中它的结果是编译时一句
        // `Permission denied (os error 13)`，而错误信息里看不出是权限问题。
        let packaged = root.join("tectonic").join(TECTONIC_BINARY);
        let source_tree = root
            .join("tectonic")
            .join(PLATFORM_SUFFIX)
            .join(TECTONIC_BINARY);
        let candidates = [source_tree, packaged];
        let tectonic = candidates
            .iter()
            .find(|path| is_executable(path))
            // 都不可执行时留下一个存在的，好让 validate 报得出"存在但不可执行"。
            .or_else(|| candidates.iter().find(|path| path.is_file()))
            .unwrap_or(&candidates[1])
            .clone();
        Self {
            tectonic,
            bundle: root.join("texbundle").join(BUNDLE_FILE_NAME),
            fonts: root.join("fonts"),
            root,
        }
    }

    fn validate(&self) -> Result<()> {
        if !self.tectonic.is_file() {
            bail!("便携式 Tectonic 不存在：{}", self.tectonic.display());
        }
        if !is_executable(&self.tectonic) {
            bail!(
                "便携式 Tectonic 没有执行权限：{}。\
                 从压缩包解压出的 runtime 需要先 chmod +x（zip 不保存 Unix 权限位）。",
                self.tectonic.display()
            );
        }
        if !self.bundle.is_file() {
            bail!("离线 TeX bundle 不存在：{}", self.bundle.display());
        }
        validate_font_dir(&self.fonts)
    }

    /// 研究报告额外需要方正、TeX Gyre Termes 和 JetBrains Mono。单独校验，
    /// 报错才说得清"更新 runtime"而不是笼统的"字体不存在"。
    pub fn validate_research_fonts(&self) -> Result<()> {
        for file in RESEARCH_FONT_FILES {
            let path = self.fonts.join(file);
            if !path.is_file() {
                bail!(
                    "研究报告字体不存在：{}。请更新到随本版发布的 runtime 包。",
                    path.display()
                );
            }
        }
        Ok(())
    }
}

/// 这个文件能不能直接执行。Windows 没有执行位的概念，存在即可。
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// 找到完整的便携式 TeX 运行时。只要发现了 Tectonic 可执行文件，其余资源缺失就明确
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
    for file in OFFICIAL_FONT_FILES {
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

/// 应用内输入法的数据目录（`dict.qj` / `lm.qj`）：跟着运行时一起发，
/// 查找规则与 TeX 运行时一致（环境变量 > 可执行文件旁 > 开发仓库）。
pub(crate) fn ime_data_roots() -> Vec<PathBuf> {
    runtime_roots()
        .into_iter()
        .map(|root| root.join("ime"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_layout_uses_stable_release_names() {
        let runtime = PortableTexRuntime::candidate(PathBuf::from("runtime"));
        assert!(
            runtime
                .tectonic
                .ends_with(format!("tectonic/{PLATFORM_SUFFIX}/{TECTONIC_BINARY}"))
                || runtime
                    .tectonic
                    .ends_with(format!("tectonic/{TECTONIC_BINARY}"))
        );
        assert!(runtime.bundle.ends_with("texbundle/gongwen-texlive.ttb"));
        assert!(runtime.fonts.ends_with("fonts"));
    }

    /// 两处都有 Tectonic 时选真正可执行的那一个。
    ///
    /// 回归测试：runtime 压缩包解压出的 `tectonic/<平台>/tectonic` 没有执行位
    /// （zip 不保存 Unix 权限位），而发布流程另外装了一份带执行位的
    /// `tectonic/tectonic`。早先无条件优先前者，结果是所有 PDF 编译都以
    /// `Permission denied (os error 13)` 失败，错误信息里还看不出是权限问题。
    #[cfg(unix)]
    #[test]
    fn picks_the_executable_tectonic_when_both_paths_exist() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temporary directory must be creatable");
        let root = dir.path();
        let platform_dir = root.join("tectonic").join(PLATFORM_SUFFIX);
        std::fs::create_dir_all(&platform_dir).unwrap();

        // 解压出来的那份：存在，但不可执行。
        let extracted = platform_dir.join(TECTONIC_BINARY);
        std::fs::write(&extracted, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&extracted, std::fs::Permissions::from_mode(0o644)).unwrap();

        // 只有它时就选它，让 validate 报得出"存在但不可执行"。
        let runtime = PortableTexRuntime::candidate(root.to_owned());
        assert_eq!(runtime.tectonic, extracted);
        let error = runtime.validate().expect_err("不可执行时必须报错");
        assert!(
            format!("{error:#}").contains("没有执行权限"),
            "错误信息要说清是权限问题：{error:#}"
        );

        // 发布流程装好的那份：带执行位，应当被优先选中。
        let installed = root.join("tectonic").join(TECTONIC_BINARY);
        std::fs::write(&installed, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            PortableTexRuntime::candidate(root.to_owned()).tectonic,
            installed
        );
    }

    /// 每个位置的内置字体都必须真的随运行时分发，否则未配置本机字体的位置会
    /// 指向一个不存在的文件，编译才报错。
    #[test]
    fn every_font_role_maps_to_a_bundled_file() {
        for role in crate::models::FontRole::ALL {
            assert!(
                font_files().any(|file| file == role.bundled_file()),
                "{} 的内置字体 {} 不在运行时字体清单里",
                role.label(),
                role.bundled_file()
            );
        }
    }

    #[test]
    fn font_dir_validation_checks_expected_filenames() {
        let dir = tempfile::tempdir().expect("temporary directory must be creatable");
        for file in font_files() {
            std::fs::write(dir.path().join(file), b"font")
                .expect("font placeholder must be writable");
        }
        validate_font_dir(dir.path()).expect("complete font directory must validate");

        std::fs::remove_file(dir.path().join(OFFICIAL_FONT_FILES[0]))
            .expect("font placeholder must be removable");
        assert!(validate_font_dir(dir.path()).is_err());
    }

    /// 研究报告字体缺失只能挡住研究报告：公文的编译与预览要照常可用，否则
    /// 一个没更新 runtime 的老安装会彻底失去出 PDF 的能力。
    #[test]
    fn missing_research_fonts_do_not_block_official_documents() {
        let dir = tempfile::tempdir().expect("temporary directory must be creatable");
        for file in OFFICIAL_FONT_FILES {
            std::fs::write(dir.path().join(file), b"font")
                .expect("font placeholder must be writable");
        }
        validate_font_dir(dir.path()).expect("公文字体齐备就应当通过校验");

        let runtime = PortableTexRuntime {
            root: dir.path().to_owned(),
            tectonic: dir.path().join(TECTONIC_BINARY),
            bundle: dir.path().join(BUNDLE_FILE_NAME),
            fonts: dir.path().to_owned(),
        };
        let error = runtime
            .validate_research_fonts()
            .expect_err("缺研究报告字体时必须报错");
        assert!(format!("{error:#}").contains("研究报告字体不存在"));

        for file in RESEARCH_FONT_FILES {
            std::fs::write(dir.path().join(file), b"font")
                .expect("font placeholder must be writable");
        }
        runtime
            .validate_research_fonts()
            .expect("补齐研究报告字体后应当通过");
    }
}
