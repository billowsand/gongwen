//! 装配拼音引擎：词库、bigram 语言模型、用户学习、附加词库。
//!
//! 这里只认 `qingjian-core` 的 `Engine` 门面，跟上游的 Windows Server 做的是同一件事，
//! 区别只是不必经过命名管道——引擎就在本进程里。

use anyhow::{Context, Result};
use qingjian_core::Engine;
use qingjian_dictionary::Dictionary;
use qingjian_learning::FrequencyLearner;
use qingjian_lm::BigramModel;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::data::ImeData;

/// 装配好的引擎，附带数据来历（设置页显示用）。
pub(crate) struct Assembly {
    /// 引擎门面。
    pub(crate) engine: Engine,

    /// 词库条数。
    pub(crate) dictionary_entries: usize,

    /// 词库名称与许可证（`.qj` 的 META 节；TSV 没有）。
    pub(crate) dictionary_name: Option<String>,
    pub(crate) dictionary_license: Option<String>,

    /// 语言模型的二元组数；没有模型时为 `None`。
    pub(crate) bigrams: Option<usize>,

    /// 这次装配花了多久（毫秒）。
    pub(crate) load_ms: u128,
}

/// 按数据装配引擎。词库读不出来是错误，其余（语言模型、学习数据、附加词库）
/// 坏了就降级：输入法能用比数据完整重要。
pub(crate) fn assemble(
    data: &ImeData,
    learning: Option<&Path>,
    extra_dicts: &Path,
) -> Result<Assembly> {
    let started = Instant::now();
    let dictionary = Dictionary::from_path(&data.dict)
        .with_context(|| format!("输入法词库读取失败：{}", data.dict.display()))?;
    let dictionary_entries = dictionary.len();
    let (dictionary_name, dictionary_license) = match dictionary.metadata() {
        Some(meta) => (
            Some(meta.name.clone()).filter(|name| !name.is_empty()),
            Some(meta.license.clone()).filter(|license| !license.is_empty()),
        ),
        None => (None, None),
    };
    let mut engine = Engine::new(dictionary);
    engine.set_extra_dictionaries(load_extra(extra_dicts));
    if let Some(dir) = learning {
        engine = engine.with_learner(Box::new(load_learner(dir)));
    }
    let mut bigrams = None;
    if let Some(path) = &data.lm {
        match BigramModel::from_path(path) {
            Ok(model) => {
                bigrams = Some(model.bigram_count());
                engine = engine.with_language_model(Box::new(model));
            }
            Err(error) => {
                eprintln!("[ime] 语言模型加载失败，整句退化成按词拼：{error}");
            }
        }
    }
    Ok(Assembly {
        engine,
        dictionary_entries,
        dictionary_name,
        dictionary_license,
        bigrams,
        load_ms: started.elapsed().as_millis(),
    })
}

/// 用户词频。读不出来（权限、坏盘）就只在内存里学习，不拿空表覆盖用户文件。
fn load_learner(dir: &Path) -> FrequencyLearner {
    let path = dir.join("user.tsv");
    match FrequencyLearner::from_path(&path) {
        Ok(learner) => learner,
        Err(error) => {
            eprintln!("[ime] 学习数据读取失败，本次只在内存里学习：{error}");
            FrequencyLearner::default()
        }
    }
}

/// 附加词库目录里的 `.qj` / TSV 全部加载。用户往这里放领域词库（公文专名、
/// 行业术语），坏文件跳过、不挡住启动。
fn load_extra(dir: &Path) -> Vec<Dictionary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("qj" | "tsv")
                )
        })
        .collect();
    // 目录顺序不稳定，排一下让"加载了哪几本"每次都一样。
    paths.sort();
    let mut dictionaries = Vec::new();
    for path in paths {
        match Dictionary::from_path(&path) {
            Ok(dictionary) => {
                eprintln!(
                    "[ime] 附加词库已加载：{}（{} 条）",
                    path.display(),
                    dictionary.len()
                );
                dictionaries.push(dictionary);
            }
            Err(error) => eprintln!("[ime] 附加词库加载失败，跳过 {}：{error}", path.display()),
        }
    }
    dictionaries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 附加词库目录不存在时安静地返回空表，不当错误。
    #[test]
    fn missing_extra_directory_is_not_an_error() {
        let dir = tempfile::tempdir().expect("临时目录");
        assert!(load_extra(&dir.path().join("nope")).is_empty());
    }

    /// 目录里的坏文件跳过，不影响其他文件。
    #[test]
    fn broken_extra_dictionary_is_skipped() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("broken.qj"), b"not a container").expect("写坏文件");
        std::fs::write(dir.path().join("ignored.txt"), b"x").expect("写无关文件");
        assert!(load_extra(dir.path()).is_empty());
    }

    /// 词库文件不存在时 `assemble` 报错，调用方据此退回系统输入法。
    #[test]
    fn missing_dictionary_fails_assembly() {
        let dir = tempfile::tempdir().expect("临时目录");
        let data = ImeData {
            dict: dir.path().join("dict.qj"),
            lm: None,
        };
        assert!(assemble(&data, None, dir.path()).is_err());
    }
}
