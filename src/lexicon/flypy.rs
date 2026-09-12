//! 小鹤双拼编码器。
//!
//! 取自同作者的 flype-word 仓库（MIT），只搬编码部分：分词、词频统计与人工
//! 修订流程在本应用里由 `lexicon::scan` 与词表页承担，不需要那边的 CLI/GUI。
//! 键位表与四码定词规则是小鹤输入法的固有约定，不随版本变化，所以这里选择
//! 复制而不是引依赖——发布流水线是离线打包的，少一个外部 git 依赖更稳妥。
//!
//! 编码规则：
//!
//! | 词长 | 规则 | 示例 |
//! |---|---|---|
//! | 1 字 | 该字完整小鹤双拼 | `小 → xn` |
//! | 2 字 | 两字各取完整双拼 | `小鹤 → xnhe` |
//! | 3 字 | 前两字声母键 + 末字完整双拼 | `知识库 → vuku` |
//! | 4 字及以上 | 前三字和末字的声母键 | `中华人民共和国 → vhrg` |
//!
//! 「声母键」是小鹤键盘上的声母键，与该音节全码的第一键始终一致：`zh/ch/sh`
//! 分别取 `v/i/u`，零声母音节取韵母的第一个字母。

use pinyin::ToPinyin;

/// 编码失败的原因。词表里逐条展示，不中断整批编码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeError(String);

impl EncodeError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// 人工标音的音节数和词的字数对不上。逐字对应是编码的前提，对不上只能报错。
    pub fn mismatch(word: &str, chars: usize, syllables: usize) -> Self {
        Self::new(format!(
            "「{word}」有 {chars} 个字，人工标音却有 {syllables} 个音节"
        ))
    }
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EncodeError {}

/// 按词长规则给整词出码。`syllables` 必须与词的字数一一对应。
pub fn encode_word_from_pinyin(syllables: &[String]) -> Result<String, EncodeError> {
    match syllables {
        [] => Err(EncodeError::new("不能编码空词语")),
        [single] => encode_syllable(single),
        [first, second] => Ok(format!(
            "{}{}",
            encode_syllable(first)?,
            encode_syllable(second)?
        )),
        [first, second, third] => Ok(format!(
            "{}{}{}",
            first_letter(first)?,
            first_letter(second)?,
            encode_syllable(third)?
        )),
        many => Ok(format!(
            "{}{}{}{}",
            first_letter(&many[0])?,
            first_letter(&many[1])?,
            first_letter(&many[2])?,
            first_letter(many.last().expect("已确认词语非空"))?
        )),
    }
}

/// 取词的默认读音。多音字一律取字典首选读音，需要纠正时由词表里的人工标音覆盖。
pub fn word_pinyin(word: &str) -> Result<Vec<String>, EncodeError> {
    word.chars()
        .map(|character| {
            character
                .to_pinyin()
                .map(|value| normalize_syllable(value.plain()))
                .ok_or_else(|| {
                    EncodeError::new(format!("“{word}”中的“{character}”没有拼音，无法编码"))
                })
        })
        .collect()
}

/// 该字符串是否整串都是能查到拼音的汉字。分词结果的入库门槛。
pub fn is_all_han(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|character| character.to_pinyin().is_some())
}

/// 单个音节的小鹤全码。
pub fn encode_syllable(raw_syllable: &str) -> Result<String, EncodeError> {
    let syllable = normalize_syllable(raw_syllable);
    if let Some(code) = zero_initial_code(&syllable) {
        return Ok(code);
    }

    let (initial_key, final_part) = split_syllable(&syllable)
        .ok_or_else(|| EncodeError::new(format!("无法识别拼音音节“{raw_syllable}”的声母")))?;
    let final_key = final_key(final_part).ok_or_else(|| {
        EncodeError::new(format!(
            "无法识别拼音音节“{raw_syllable}”的韵母“{final_part}”"
        ))
    })?;

    Ok(format!("{initial_key}{final_key}"))
}

/// 零声母音节（拼音以韵母开头）的打法：
///
/// - 单字母韵母：重复两次该键，`啊 a → aa`、`哦 o → oo`、`额 e → ee`；
/// - 双字母韵母：直接打全拼，`爱 ai → ai`、`恩 en → en`、`欧 ou → ou`、`儿 er → er`；
/// - 三字母韵母：韵母首字母 + 韵母键，`昂 ang → ah`、`鞥 eng → eg`。
///
/// 不是零声母音节（或不是合法的零声母韵母）时返回 `None`。
fn zero_initial_code(syllable: &str) -> Option<String> {
    const ZERO_INITIAL_FINALS: [&str; 12] = [
        "a", "o", "e", "ai", "ei", "ao", "ou", "an", "en", "er", "ang", "eng",
    ];
    if !ZERO_INITIAL_FINALS.contains(&syllable) {
        return None;
    }

    let first = syllable.chars().next()?;
    match syllable.len() {
        1 => Some(format!("{first}{first}")),
        2 => Some(syllable.to_owned()),
        _ => final_key(syllable).map(|key| format!("{first}{key}")),
    }
}

/// 韵母在小鹤双拼键盘上的键位。
fn final_key(final_part: &str) -> Option<char> {
    let normalized_final = match final_part {
        "iou" => "iu",
        "uei" => "ui",
        "uen" => "un",
        "üe" => "ve",
        value => value,
    };
    Some(match normalized_final {
        "a" => 'a',
        "o" => 'o',
        "e" => 'e',
        "i" => 'i',
        "u" => 'u',
        "v" | "ü" => 'v',
        "ai" => 'd',
        "ei" => 'w',
        "ui" => 'v',
        "ao" => 'c',
        "ou" => 'z',
        "iu" => 'q',
        "ie" => 'p',
        "ue" | "ve" => 't',
        "er" => 'r',
        "an" => 'j',
        "en" => 'f',
        "in" => 'b',
        "un" | "vn" => 'y',
        "ang" => 'h',
        "eng" => 'g',
        "ing" => 'k',
        "ong" | "iong" => 's',
        "ia" | "ua" => 'x',
        "ian" => 'm',
        "uan" => 'r',
        "iang" | "uang" => 'l',
        "iao" => 'n',
        "uai" => 'k',
        "uo" => 'o',
        _ => return None,
    })
}

/// 拆出音节的小鹤声母键和剩下的韵母。
///
/// 小鹤双拼里 `zh`/`ch`/`sh` 打在 `v`/`i`/`u` 上，所以声母键不一定等于拼音首字母。
/// 零声母音节（`an`、`ou` 等）没有声母，返回 `None`。
fn split_syllable(syllable: &str) -> Option<(char, &str)> {
    for (initial, key) in [("zh", 'v'), ("ch", 'i'), ("sh", 'u')] {
        if let Some(final_part) = syllable.strip_prefix(initial) {
            return Some((key, final_part));
        }
    }
    let first = syllable.chars().next()?;
    if "bpmfdtnlgkhjqxrzcsyw".contains(first) {
        Some((first, &syllable[first.len_utf8()..]))
    } else {
        None
    }
}

/// 词组编码里“取首字母”用的那一键。
///
/// 必须和全码的第一键一致：`知/识` 的全码是 `vi`/`ui`，所以 `知识库` 是 `vuku`
/// 而不是按拼音首字母拼出来的 `zsku`。零声母音节取韵母的第一个字母，
/// 这本来就等于它全码的第一键。
fn first_letter(raw_syllable: &str) -> Result<char, EncodeError> {
    let syllable = normalize_syllable(raw_syllable);
    if let Some((key, _)) = split_syllable(&syllable) {
        return Ok(key);
    }
    syllable
        .chars()
        .next()
        .filter(|character| character.is_ascii_lowercase())
        .ok_or_else(|| EncodeError::new(format!("无法取得拼音“{raw_syllable}”的首字母")))
}

/// 归一化一个音节：去声调数字、`ü` 统一写成 `v`。
pub fn normalize_syllable(raw_syllable: &str) -> String {
    raw_syllable
        .trim()
        .to_lowercase()
        .replace("u:", "v")
        .replace('ü', "v")
        .trim_end_matches(|character: char| ('0'..='5').contains(&character))
        .to_owned()
}

/// 解析人工标注的一串拼音，音节之间允许空格、逗号、顿号、斜杠、分号分隔。
pub fn parse_pinyin(text: &str) -> Vec<String> {
    text.split(|character: char| {
        character.is_whitespace() || matches!(character, ',' | '，' | '/' | '、' | ';' | '；')
    })
    .filter(|value| !value.is_empty())
    .map(normalize_syllable)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syllables(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn encodes_xiaohe_syllables() {
        let cases = [
            ("xiao", "xn"),
            ("he", "he"),
            ("shuang", "ul"),
            ("pin", "pb"),
            ("yin", "yb"),
            ("zhong", "vs"),
            ("lüe", "lt"),
            ("ju", "ju"),
            ("yuan", "yr"),
            ("ang", "ah"),
        ];
        for (pinyin, expected) in cases {
            assert_eq!(encode_syllable(pinyin).unwrap(), expected);
        }
    }

    #[test]
    fn applies_word_length_rules() {
        assert_eq!(
            encode_word_from_pinyin(&syllables(&["xiao", "he"])).unwrap(),
            "xnhe"
        );
        assert_eq!(
            encode_word_from_pinyin(&syllables(&["ji", "suan", "ji"])).unwrap(),
            "jsji"
        );
        assert_eq!(
            encode_word_from_pinyin(&syllables(&["shuang", "pin", "fang", "an"])).unwrap(),
            "upfa"
        );
        assert_eq!(
            encode_word_from_pinyin(&syllables(&[
                "zhong", "hua", "ren", "min", "gong", "he", "guo"
            ]))
            .unwrap(),
            "vhrg"
        );
    }

    #[test]
    fn zero_initial_syllables_follow_the_length_rules() {
        for (syllable, expected) in [("a", "aa"), ("o", "oo"), ("e", "ee")] {
            assert_eq!(encode_syllable(syllable).unwrap(), expected);
        }
        for syllable in ["ai", "ei", "ao", "ou", "an", "en", "er"] {
            assert_eq!(encode_syllable(syllable).unwrap(), syllable);
        }
        assert_eq!(encode_syllable("ang").unwrap(), "ah");
        assert_eq!(encode_syllable("eng").unwrap(), "eg");
        // 有声母时同样的韵母走键位表：海 hai → hd，而零声母的 爱 ai → ai。
        assert_eq!(encode_syllable("hai").unwrap(), "hd");
        assert_eq!(encode_syllable("gei").unwrap(), "gw");
        assert!(encode_syllable("ez").is_err());
        assert!(encode_syllable("aq").is_err());
    }

    #[test]
    fn word_codes_use_the_xiaohe_initial_key_for_zh_ch_sh() {
        assert_eq!(
            encode_word_from_pinyin(&syllables(&["zhi", "shi", "ku"])).unwrap(),
            "vuku"
        );
        assert_eq!(
            encode_word_from_pinyin(&syllables(&["chong", "qing", "shi"])).unwrap(),
            "iqui"
        );
        // 首字母必须和该音节全码的第一键一致。
        for syllable in [
            "zhi", "chi", "shi", "zha", "chuang", "shuo", "si", "ci", "zi",
        ] {
            let full = encode_syllable(syllable).unwrap();
            assert_eq!(
                first_letter(syllable).unwrap(),
                full.chars().next().unwrap(),
                "音节 {syllable} 的首字母和全码 {full} 的第一键不一致"
            );
        }
    }

    #[test]
    fn reads_default_pronunciation_from_the_dictionary() {
        assert_eq!(word_pinyin("公文").unwrap(), syllables(&["gong", "wen"]));
        assert!(word_pinyin("公文A").is_err());
        assert!(is_all_han("公文"));
        assert!(!is_all_han("公文A"));
        assert!(!is_all_han(""));
    }

    #[test]
    fn parses_manual_pinyin_separators() {
        assert_eq!(parse_pinyin("chong qing"), syllables(&["chong", "qing"]));
        assert_eq!(parse_pinyin("zhong4、qing4"), syllables(&["zhong", "qing"]));
        assert_eq!(parse_pinyin("lüe/ju"), syllables(&["lve", "ju"]));
    }
}
