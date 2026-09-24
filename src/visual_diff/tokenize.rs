//! 词级 token：jieba 分词，日期、数字 + 单位、文号、公式整体为一个 token
//! （方案规则 3）。比较用的键忽略空白（规则 5），原文保留在 token 里。

use regex::Regex;
use std::sync::{Mutex, OnceLock};

/// 一个 token：原文 `text` 与比较键 `key`（去掉空白）。`start/end` 是原文
/// 字节范围，供把 diff 结果切回段落。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) text: String,
    pub(crate) key: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// 公式（`$$...$$` / `$...$`）。
static FORMULA_RE: OnceLock<Regex> = OnceLock::new();
/// 日期：「2026年9月」「8月10日」「二〇二六年九月二十四日」。
static DATE_RE: OnceLock<Regex> = OnceLock::new();
/// 数字 + 单位：「50万元」「3个工作日」「15%」。
static NUMBER_UNIT_RE: OnceLock<Regex> = OnceLock::new();
/// 文号：「某字〔2026〕3号」。
static DOCUMENT_NUMBER_RE: OnceLock<Regex> = OnceLock::new();
/// 连续半角空白（比较键里剥掉）。
static SPACES_RE: OnceLock<Regex> = OnceLock::new();

fn formula_re() -> &'static Regex {
    FORMULA_RE.get_or_init(|| Regex::new(r"\$\$.+?\$\$|\$[^$]+?\$").expect("公式正则"))
}

/// 文本里第一个公式（`$$…$$` / `$…$`）的字节范围。与分词用同一条正则：
/// 分词认作一个整体 token 的，序列化时也按公式原样写。
pub(crate) fn find_formula(text: &str) -> Option<std::ops::Range<usize>> {
    formula_re().find(text).map(|found| found.range())
}

fn date_re() -> &'static Regex {
    DATE_RE.get_or_init(|| {
        Regex::new(concat!(
            r"(?:[0-9]{4}\s*年\s*[0-9]{1,2}\s*月(?:\s*[0-9]{1,2}\s*日)?",
            r"|[0-9]{1,2}\s*月\s*[0-9]{1,2}\s*日",
            r"|[〇零一二三四五六七八九十]{4}年[〇零一二三四五六七八九十]+月(?:[〇零一二三四五六七八九十]+日)?)"
        ))
        .expect("日期正则")
    })
}

fn number_unit_re() -> &'static Regex {
    NUMBER_UNIT_RE.get_or_init(|| {
        Regex::new(concat!(
            r"[0-9]+(?:\.[0-9]+)?\s*(?:",
            "万元|亿元|万|千米|公里|千克|公斤|厘米|毫米|平方米|平方公里|",
            "个|项|件|份|次|家|人|元|吨|克|米|年|月|日|天|小时|分钟|%",
            ")"
        ))
        .expect("数字单位正则")
    })
}

fn document_number_re() -> &'static Regex {
    DOCUMENT_NUMBER_RE
        .get_or_init(|| Regex::new(r"[一-龥A-Za-z]{2,10}〔[0-9]{4}〕[0-9]+号?").expect("文号正则"))
}

fn spaces_re() -> &'static Regex {
    SPACES_RE.get_or_init(|| Regex::new(r"\s+").expect("空白正则"))
}

/// 受保护的整段 span（公式 / 日期 / 数字单位 / 文号），按出现顺序、互不重叠。
fn protected_spans(text: &str) -> Vec<(usize, usize)> {
    // 优先级：公式 > 日期 > 数字单位 > 文号；同级别先出现者优先。
    const PATTERNS: [fn() -> &'static Regex; 4] =
        [formula_re, date_re, number_unit_re, document_number_re];
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for pattern in PATTERNS {
        for found in pattern().find_iter(text) {
            let span = (found.start(), found.end());
            if spans
                .iter()
                .any(|other| span.0 < other.1 && other.0 < span.1)
            {
                continue;
            }
            spans.push(span);
        }
    }
    spans.sort_by_key(|span| span.0);
    spans
}

static JIEBA: OnceLock<Mutex<jieba_rs::Jieba>> = OnceLock::new();

fn jieba() -> &'static Mutex<jieba_rs::Jieba> {
    JIEBA.get_or_init(|| Mutex::new(jieba_rs::Jieba::new()))
}

/// 把一段正文切成词级 token。
pub(crate) fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let protected = protected_spans(text);
    let mut cursor = 0usize;
    for (start, end) in protected {
        tokens.extend(jieba_tokens(&text[cursor..start], cursor));
        tokens.push(Token {
            key: normalize(&text[start..end]),
            text: text[start..end].to_string(),
            start,
            end,
        });
        cursor = end;
    }
    tokens.extend(jieba_tokens(&text[cursor..], cursor));
    tokens
}

/// 比较键：去掉全部空白（方案规则 5：空格不标）。
fn normalize(text: &str) -> String {
    spaces_re().replace_all(text, "").to_string()
}

/// 普通区间走 jieba；相邻的纯西文字符 token 再并成一串（「GDP」「3D」不拆）。
fn jieba_tokens(text: &str, base: usize) -> Vec<Token> {
    if text.is_empty() {
        return Vec::new();
    }
    let cut = jieba().lock().expect("jieba 锁").cut(text, false);
    let mut tokens: Vec<Token> = Vec::new();
    for piece in cut {
        let start = base + piece.byte_start;
        let end = base + piece.byte_end;
        let word = piece.word;
        let ascii = word.chars().all(|ch| ch.is_ascii_alphanumeric());
        if ascii
            && let Some(last) = tokens.last_mut()
            && last.text.chars().all(|ch| ch.is_ascii_alphanumeric())
        {
            last.text.push_str(word);
            last.key.push_str(word);
            last.end = end;
            continue;
        }
        tokens.push(Token {
            key: normalize(word),
            text: word.to_string(),
            start,
            end,
        });
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<String> {
        tokenize(text).into_iter().map(|token| token.key).collect()
    }

    #[test]
    fn plain_chinese_is_cut_into_words() {
        let keys = keys("同意你单位关于报送的请示。");
        assert_eq!(
            keys,
            ["同意", "你", "单位", "关于", "报送", "的", "请示", "。"]
        );
    }

    #[test]
    fn a_date_is_one_token() {
        // 「8月10日」改成「8月15日」要整体删旧插新，不能只标「0→5」。
        assert_eq!(
            keys("请于8月10日前报送。"),
            ["请", "于", "8月10日", "前", "报送", "。"]
        );
        assert_eq!(
            keys("二〇二六年九月报送。"),
            ["二〇二六年九月", "报送", "。"]
        );
    }

    #[test]
    fn a_number_with_unit_is_one_token() {
        assert_eq!(keys("拨付50万元。"), ["拨付", "50万元", "。"]);
        assert_eq!(keys("于8月印发。"), ["于", "8月", "印发", "。"]);
        assert_eq!(keys("不超过15%。"), ["不", "超过", "15%", "。"]);
    }

    #[test]
    fn a_document_number_is_one_token() {
        assert_eq!(
            keys("按某政办〔2026〕3号文件执行。"),
            ["按某政办〔2026〕3号", "文件", "执行", "。"]
        );
    }

    #[test]
    fn a_formula_is_one_token() {
        assert_eq!(keys("由$x^{2}+1$可知。"), ["由", "$x^{2}+1$", "可知", "。"]);
        assert_eq!(keys("$$E=mc^2$$"), ["$$E=mc^2$$"]);
    }

    #[test]
    fn comparison_keys_ignore_spaces() {
        let tokens = tokenize("8 月 10 日");
        assert_eq!(tokens.len(), 1, "带空白的日期仍是整 token：{tokens:?}");
        assert_eq!(tokens[0].key, "8月10日");
    }
}
