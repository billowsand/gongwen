//! 随包运行时定位：字体与输入法数据。
//!
//! 发布包把字体、输入法词库放在可执行文件旁的 `runtime` 目录；开发构建另查仓库
//! 根目录，方便直接运行测试而不污染 PATH。PDF 由 Typst 在进程内排版，不再需要
//! 外部排版程序。

use anyhow::{Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 公文文种排版所需的字体，Typst 公文模板按文件加载。
///
/// 这一组是**全部** PDF 排版和纸面预览的下限：缺任何一个都排不出公文，因此
/// `find_font_dir` 按它校验。
///
/// 中文字形统一用字符覆盖到 GBK 的方正系列；两个 GW*Latin 子集只含可打印
/// ASCII，来自仿宋_GB2312 与宋体，由 `scripts/make-latin-subsets.py` 生成，
/// 在字体回退链最前面接管正文的西文/数字与页码数字，保持国标字面。
pub const OFFICIAL_FONT_FILES: &[&str] = &[
    "FZFangSong.ttf",
    "FZKai.ttf",
    "FZHei.ttf",
    "FZShuSong.ttf",
    "XiaoBiaoSong.ttf",
    "GWFangSongLatin.ttf",
    "GWSimSunLatin.ttf",
];

/// 研究报告额外需要的字体（TeX Gyre Termes、JetBrains Mono），研究报告模板按
/// 文件名取家族名（`export::typst::research`），**改名要两边一起改**。
/// 方正四款与公文共用，在 [`OFFICIAL_FONT_FILES`] 里。
///
/// 单独成组是因为它只挡研究报告：老 runtime 目录缺这几个字体时，公文照常
/// 排版和预览，只有研究报告报错，不至于让整台机器失去出 PDF 的能力。
pub const RESEARCH_FONT_FILES: &[&str] = &[
    "JetBrainsMono-Regular.ttf",
    "texgyretermes-regular.otf",
    "texgyretermes-bold.otf",
    "texgyretermes-italic.otf",
    "texgyretermes-bolditalic.otf",
];

/// 正文西文/数字的拉丁子集（仿宋_GB2312 字面），在正文回退链最前面。
pub const BODY_LATIN_SUBSET_FILE: &str = "GWFangSongLatin.ttf";
/// 页码数字的拉丁子集（宋体字面），在页码回退链最前面。
pub const PAGE_NUMBER_LATIN_SUBSET_FILE: &str = "GWSimSunLatin.ttf";

/// 两组字体的并集，发布包必须齐备。
pub fn font_files() -> impl Iterator<Item = &'static str> {
    OFFICIAL_FONT_FILES
        .iter()
        .chain(RESEARCH_FONT_FILES)
        .copied()
}

/// 公文排版与预览只需要字体：先找随包 runtime，再退到仓库 `font` 目录中的开发资产。
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

/// 研究报告额外需要方正、TeX Gyre Termes 和 JetBrains Mono。单独校验，报错才说得清
/// "更新 runtime"而不是笼统的"字体不存在"。
pub fn validate_research_fonts(fonts: &Path) -> Result<()> {
    for file in RESEARCH_FONT_FILES {
        let path = fonts.join(file);
        if !path.is_file() {
            bail!(
                "研究报告字体不存在：{}。请更新到随本版发布的 runtime 包。",
                path.display()
            );
        }
    }
    Ok(())
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

    /// 每个位置的内置字体都必须真的随运行时分发，否则未配置本机字体的位置会
    /// 指向一个不存在的文件，排版才报错。
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

    /// 研究报告字体缺失只能挡住研究报告：公文的排版与预览要照常可用，否则
    /// 一个没更新 runtime 的老安装会彻底失去出 PDF 的能力。
    #[test]
    fn missing_research_fonts_do_not_block_official_documents() {
        let dir = tempfile::tempdir().expect("temporary directory must be creatable");
        for file in OFFICIAL_FONT_FILES {
            std::fs::write(dir.path().join(file), b"font")
                .expect("font placeholder must be writable");
        }
        validate_font_dir(dir.path()).expect("公文字体齐备就应当通过校验");
        let error = validate_research_fonts(dir.path()).expect_err("缺研究报告字体时必须报错");
        assert!(format!("{error:#}").contains("研究报告字体不存在"));

        for file in RESEARCH_FONT_FILES {
            std::fs::write(dir.path().join(file), b"font")
                .expect("font placeholder must be writable");
        }
        validate_research_fonts(dir.path()).expect("补齐研究报告字体后应当通过");
    }
}
