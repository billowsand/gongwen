//! 输入法数据文件的位置。
//!
//! 词库 `dict.qj` 与 bigram 语言模型 `lm.qj` 是随包资源：发布包里跟 TeX 运行时
//! 同一个目录树（`runtime/ime/`），开发时放仓库的 `runtime/ime/`。两处都没有就
//! 不启用应用内输入法，用户继续用系统输入法——缺数据不该挡住打字。

use std::path::{Path, PathBuf};

/// 词库文件名。
const DICT_FILE: &str = "dict.qj";

/// 语言模型文件名。
const LM_FILE: &str = "lm.qj";

/// 一份可用的输入法数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImeData {
    /// 拼音词库，必需：缺了就只有零候选。
    pub(crate) dict: PathBuf,

    /// bigram 语言模型，可选：缺了整句转换退化成按词拼，词级候选照常。
    pub(crate) lm: Option<PathBuf>,
}

impl ImeData {
    /// 从一个目录里取数据；词库不在就当这个目录没有数据。
    fn from_dir(dir: &Path) -> Option<Self> {
        let dict = dir.join(DICT_FILE);
        if !dict.is_file() {
            return None;
        }
        let lm = dir.join(LM_FILE);
        Some(Self {
            dict,
            lm: lm.is_file().then_some(lm),
        })
    }
}

/// 找一份输入法数据。`GONGWEN_IME_DATA` 指定了目录就以它为准（指错就当没有，
/// 免得调试时悄悄用了另一份）；否则按运行时目录的查找顺序来。
pub(crate) fn find() -> Option<ImeData> {
    if let Some(dir) = std::env::var_os("GONGWEN_IME_DATA") {
        return ImeData::from_dir(Path::new(&dir));
    }
    crate::portable_runtime::ime_data_roots()
        .iter()
        .find_map(|dir| ImeData::from_dir(dir))
}

/// 用户学习数据目录：与稿件库同一个用户目录（`config_dir()/ime`），词频、用户词、
/// 个人 n-gram、敲错表都写在这里，删掉这个目录就等于让输入法忘掉使用者。
///
/// 建不出来就当不学习：输入法照常可用，只是这次运行记不住选词。
pub(crate) fn learning_dir() -> Option<PathBuf> {
    let dir = crate::storage::config_dir().ok()?.join("ime");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只有词库时也算一份数据，但语言模型留空。
    #[test]
    fn dictionary_alone_is_enough() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join(DICT_FILE), b"x").expect("写词库");
        let data = ImeData::from_dir(dir.path()).expect("应当认得这份数据");
        assert_eq!(data.dict.file_name().unwrap(), DICT_FILE);
        assert!(data.lm.is_none());
    }

    /// 没有词库的目录不算数据。
    #[test]
    fn missing_dictionary_means_no_data() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join(LM_FILE), b"x").expect("写语言模型");
        assert!(ImeData::from_dir(dir.path()).is_none());
    }

    /// 两份都在时语言模型也带上。
    #[test]
    fn language_model_is_picked_up_when_present() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join(DICT_FILE), b"x").expect("写词库");
        std::fs::write(dir.path().join(LM_FILE), b"x").expect("写语言模型");
        let data = ImeData::from_dir(dir.path()).expect("应当认得这份数据");
        assert_eq!(
            data.lm.as_deref().and_then(Path::file_name),
            Some(LM_FILE.as_ref())
        );
    }
}
