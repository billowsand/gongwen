//! 全应用共用的一个 jieba 实例，可以挂上公文词表当用户词典。
//!
//! 这是词表回喂给应用自己的那一路。在此之前，`rag.rs` 的关键词召回和
//! `export/title.rs` 的标题断行各自 `OnceLock` 一个默认 jieba，都不认识
//! 「新舆处」「XX 专项整治行动」这类本单位专名，会把它们切碎——检索因此漏召，
//! 标题换行也可能从词中间断开。词表一建起来，两处同时受益。
//!
//! 只有**已接受**的词进用户词典：待确认的候选词还没人看过，让它们影响分词
//! 等于把噪声固化。

use jieba_rs::Jieba;
use std::sync::{Arc, OnceLock, RwLock};

/// 载入 jieba 自带词典要几十毫秒，所以这里只在真正用到时装一次。
static SHARED: OnceLock<RwLock<Arc<Jieba>>> = OnceLock::new();

fn cell() -> &'static RwLock<Arc<Jieba>> {
    SHARED.get_or_init(|| RwLock::new(Arc::new(Jieba::new())))
}

/// 取当前分词器。首次调用会载入 jieba 自带词典，之后是一次 `Arc` 克隆。
pub fn shared() -> Arc<Jieba> {
    cell()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 换上带用户词典的分词器。`dict` 是 jieba 词典文本（每行「词 词频」）。
///
/// 整个换掉而不是往现有实例里加词：`Jieba` 的加词要 `&mut`，而分词端拿的是
/// 共享引用；重建一个再原子替换，正在分词的调用方继续用旧的那个，不会阻塞。
/// 空词典等于回到自带词典。
pub fn install_user_dict(dict: &str) {
    let mut jieba = Jieba::new();
    if !dict.trim().is_empty() {
        let mut reader = std::io::Cursor::new(dict.as_bytes());
        // 用户词典解析失败（某行词频不是整数）时保留自带词典，不让分词整个失效。
        if jieba.load_dict(&mut reader).is_err() {
            return;
        }
    }
    // 启动时这通常是第一次接触分词器：直接把建好的这个装进去，
    // 走 `cell()` 会先装一遍自带词典再扔掉，白花一次载入时间。
    let jieba = Arc::new(jieba);
    if SHARED.set(RwLock::new(jieba.clone())).is_ok() {
        return;
    }
    if let Ok(mut guard) = cell().write() {
        *guard = jieba;
    }
}

/// 启动时挂上词表。词表还空着（库刚建、还没扫过）时什么也不做，
/// 让分词器保持惰性——没用到分词的那一次启动不必为它付载入时间。
pub fn install_from_store(store: &super::LexiconStore) {
    if let Ok(dict) = store.jieba_user_dict()
        && !dict.trim().is_empty()
    {
        install_user_dict(&dict);
    }
}

/// 把文本切成空格分隔的词串，供 FTS5 索引与查询使用。
pub fn tokenize(text: &str) -> String {
    shared()
        .cut(text, true)
        .into_iter()
        .map(|token| token.word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 切出词列表，供标题断行这类需要逐词处理的场合使用。
pub fn words(text: &str) -> Vec<String> {
    shared()
        .cut(text, true)
        .into_iter()
        .map(|token| token.word.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_separates_words_with_spaces() {
        let tokens = tokenize("现将有关事项通知如下");
        assert!(tokens.contains(' '), "jieba 分词应产生空格分隔：{tokens}");
    }

    #[test]
    fn user_dict_keeps_a_local_abbreviation_together() {
        // 自带词典不认识这个简称，会把它切碎；挂上用户词典后应整词切出。
        let coined = "新舆处";
        let before = words(&format!("请{coined}按期报送"));
        install_user_dict(&format!("{coined} 50\n"));
        let after = words(&format!("请{coined}按期报送"));
        // 换回自带词典，免得影响同进程里的其他测试。
        install_user_dict("");
        assert!(
            after.iter().any(|word| word == coined),
            "挂上用户词典后应整词切出 {coined}：{after:?}"
        );
        assert!(
            !before.iter().any(|word| word == coined),
            "这个用例要证明用户词典起了作用，前提是自带词典切不出它：{before:?}"
        );
    }

    #[test]
    fn a_broken_user_dict_does_not_destroy_segmentation() {
        install_user_dict("新舆处 不是数字\n");
        let tokens = tokenize("现将有关事项通知如下");
        install_user_dict("");
        assert!(!tokens.is_empty());
    }
}
