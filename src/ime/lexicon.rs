//! 公文词表 → 输入法的附加词库。
//!
//! 公文助手自己攒的词表（本单位专名、公文套语、行业术语）比任何通用输入法的
//! 词库都更懂这个单位在写什么。把它导出成青简词库 TSV 交给应用内输入法，是
//! 「为什么要自带输入法」最直接的理由：系统输入法永远不知道「某某处」「某某函」
//! 该怎么打。
//!
//! 输出格式与 `vendor/qingjian/dictionary/src/lib.rs` 一致：
//! `词\t音节（空格分隔）\t词频`，`#` 开头的行是注释、空行忽略。
//! 音节写法与 `flypy` 的规范化一致（`lüe` → `lve`），也正是引擎的
//! `canonical_syllable` 认的那一套。

use crate::lexicon::{LexiconTerm, flypy};

/// 导出的文件名，落在 `config_dir()/ime/dicts/` 下。
pub(crate) const FILE_NAME: &str = "gongwen-lexicon.tsv";

/// 单字不进附加词库：基础词库里都有，再塞一遍只会让它的词频叠一层。
const MIN_CHARS: usize = 2;

/// 附加词库的词频上限。再高频的专名也不该压过基础词库里的常用词。
const MAX_FREQUENCY: i64 = 30_000;

/// 把词表写成附加词库。返回（文本，写进去的条数）。
pub(crate) fn build(terms: &[LexiconTerm]) -> (String, usize) {
    let mut out = String::from(
        "# 由公文助手自动导出：词表里的词、读音与词频。\n\
         # 请勿手工编辑，下次同步会整份覆盖；要加词请加到应用的词表页。\n",
    );
    let mut written = 0;
    for term in terms {
        if term.char_count() < MIN_CHARS {
            continue;
        }
        let Some(syllables) = reading(term) else {
            continue;
        };
        out.push_str(&term.term);
        out.push('\t');
        out.push_str(&syllables.join(" "));
        out.push('\t');
        out.push_str(&frequency(term).to_string());
        out.push('\n');
        written += 1;
    }
    (out, written)
}

/// 词的读音：有人工标注就用标注的，否则取字典默认读音（多音字取首选）。
///
/// 取不到读音的词跳过——没有读音就查不出来，写进去只占地方。
fn reading(term: &LexiconTerm) -> Option<Vec<String>> {
    let syllables = if term.pinyin.trim().is_empty() {
        flypy::word_pinyin(&term.term).ok()?
    } else {
        flypy::parse_pinyin(&term.pinyin)
    };
    (!syllables.is_empty()).then_some(syllables)
}

/// 附加词库里的词频。
///
/// 主信号取「出现篇数」而不是出现次数：同一篇稿子里反复出现的词（通篇的单位简称）
/// 不该压过跨多篇都常见的词。
fn frequency(term: &LexiconTerm) -> i64 {
    (500 + term.doc_count * 200 + term.freq_total * 20).clamp(1, MAX_FREQUENCY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexicon::{TermOrigin, TermState};

    /// 造一条词，只关心与导出有关的那几项。
    fn term(text: &str, pinyin: &str, freq_total: i64, doc_count: i64) -> LexiconTerm {
        LexiconTerm {
            id: 1,
            term: text.to_owned(),
            pinyin: pinyin.to_owned(),
            code_override: String::new(),
            freq_total,
            doc_count,
            origin: TermOrigin::Corpus,
            state: TermState::Accepted,
            locked: false,
            in_base_dict: false,
            group_name: String::new(),
            note: String::new(),
            first_seen: String::new(),
            last_seen: String::new(),
        }
    }

    /// 导出的行是「词 / 音节 / 词频」，注释行开头是 `#`。
    #[test]
    fn writes_word_syllables_and_frequency() {
        let (tsv, written) = build(&[term("综合处", "zong he chu", 12, 3)]);
        assert_eq!(written, 1);
        let line = tsv
            .lines()
            .find(|line| line.starts_with("综合处"))
            .expect("应当有综合处这一行");
        assert_eq!(line, "综合处\tzong he chu\t1340");
    }

    /// 没标读音的词用字典默认读音，多音字取首选。
    #[test]
    fn falls_back_to_the_dictionary_reading() {
        let (tsv, written) = build(&[term("公文", "", 1, 0)]);
        assert_eq!(written, 1);
        assert!(tsv.contains("公文\tgong wen\t"), "{tsv}");
    }

    /// `lüe` 这类音节按引擎认的写法写成 `lve`。
    #[test]
    fn normalizes_syllables_the_way_the_engine_expects() {
        let (tsv, _) = build(&[term("策略", "ce lüe", 1, 0)]);
        assert!(tsv.contains("策略\tce lve\t"), "{tsv}");
    }

    /// 单字不进附加词库，基础词库里都有。
    #[test]
    fn skips_single_characters() {
        let (_, written) = build(&[term("我", "", 100, 50)]);
        assert_eq!(written, 0);
    }

    /// 取不到读音的词跳过，不写半条。
    #[test]
    fn skips_terms_without_a_reading() {
        let (_, written) = build(&[term("A4纸", "", 1, 0)]);
        assert_eq!(written, 0);
    }

    /// 词频以篇数为主信号，并封顶。
    #[test]
    fn frequency_weights_document_count_and_is_capped() {
        // 一篇文章里出现一次（+200）重于同一篇里出现五次（+100）。
        assert!(frequency(&term("甲", "", 0, 1)) > frequency(&term("乙", "", 5, 0)));
        assert_eq!(
            frequency(&term("丙", "", 1_000_000, 1_000_000)),
            MAX_FREQUENCY
        );
    }

    /// 注释行以 `#` 开头（引擎按注释跳过），且不影响条数。
    #[test]
    fn header_is_a_comment() {
        let (tsv, written) = build(&[]);
        assert_eq!(written, 0);
        assert!(tsv.lines().all(|line| line.starts_with('#')));
    }
}
