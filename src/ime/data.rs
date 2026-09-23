//! 输入法数据文件的位置。
//!
//! 词库 `dict.qj` 与 bigram 语言模型 `lm.qj` 是随包资源：发布包里跟 TeX 运行时
//! 同一个目录树（`runtime/ime/`），开发时放仓库的 `runtime/ime/`。两处都没有就
//! 不启用应用内输入法，用户继续用系统输入法——缺数据不该挡住打字。

use std::path::{Path, PathBuf};

use qingjian_core::FumaScheme;

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

/// 附加词库目录：公文词表导出的 TSV 落在这里，用户也可以自己往里丢领域词库
/// （`.qj` 或青简 TSV，见 `vendor/qingjian/dictionary/src/lib.rs`）。
pub(crate) fn dicts_dir() -> Option<PathBuf> {
    let dir = learning_dir()?.join("dicts");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// 辅码表的存放路径（用户自己导入的）。
///
/// **不随包分发**：小鹤辅码表的权利归方案作者，上游未取得再分发授权，
/// 所以只能由使用者自己导入一份（格式与上游 `assets/fuma/xiaohe.txt` 一致，
/// 每行 `字=两码`）。文件不在就当辅码关着。
pub(crate) fn fuma_path(scheme: FumaScheme) -> Option<PathBuf> {
    let file = Path::new(scheme.asset()).file_name()?;
    let dir = learning_dir()?.join("fuma");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(file))
}

/// 当前构建是否打包了「内置示例小鹤辅码」内容。
///
/// 见 `Cargo.toml` 的 `ime-builtin-xiaohe` feature：默认关闭（发布构建必关），
/// 关闭时本函数返回 `false`，且二进制里**不含**这份码表的内容——避免无意中
/// 把权利受限的第三方表随包分发出去。开发者本地 `cargo run` 可以加
/// `--features ime-builtin-xiaohe` 打开。
pub(crate) const fn has_builtin_xiaohe() -> bool {
    cfg!(feature = "ime-builtin-xiaohe")
}

/// 内置示例小鹤辅码表的内容（仅 feature 开启时才有）。
///
/// 返回值是编译期常量字符串，二进制里**没有**这份内容则返回 `None`。
/// 详见 [`has_builtin_xiaohe`] 的说明。
pub(crate) fn builtin_xiaohe() -> Option<&'static str> {
    #[cfg(feature = "ime-builtin-xiaohe")]
    {
        Some(include_str!("../../assets/fuma/xiaohe.txt"))
    }
    #[cfg(not(feature = "ime-builtin-xiaohe"))]
    {
        None
    }
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
