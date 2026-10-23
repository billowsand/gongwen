//! 段首编号的识别。
//!
//! 编号样式（`第X章`、`一、`、`（一）`、`1.`、`（1）`、`①`）从浅到深定层级；层级只决定
//! 「大概多深」，最终排成几级由 `structure` 的 `heading_level_map` 按全篇实际用到的层级
//! 压紧。逐行识别见 [`parse_prefix`]。

use regex::Regex;
use std::sync::OnceLock;

// ---------------------------------------------------------------- 编号

pub(super) struct Prefix {
    pub(super) level: u8,
    pub(super) number: u32,
    /// 编号（连同其后空白）占的字节数。
    pub(super) len: usize,
    pub(super) ambiguous: bool,
}

/// 认段首编号。`from_list` 表示编号来自 Word 自动编号：汉字计数的「一、」会被
/// 转成「1、」，所以这时 `N、` 算一级标题。
pub(super) fn parse_prefix(text: &str, from_list: bool) -> Option<Prefix> {
    static PATTERNS: OnceLock<[Regex; 7]> = OnceLock::new();
    let [
        chapter,
        chinese,
        chinese_paren,
        arabic,
        arabic_paren,
        arabic_stop,
        circled,
    ] = PATTERNS.get_or_init(|| {
        let numeral = "[一二三四五六七八九十百零〇]{1,4}";
        let digits = "[0-9０-９]{1,3}";
        [
            // 第X章 / 第X节 / 第X部分 / 第X编：比「一、」还浅的一层。
            Regex::new(
                r"^第([一二三四五六七八九十百零〇0-9０-９]{1,4})(章|节|部分|编)\s*[、.．:：]?\s*",
            ),
            Regex::new(&format!(r"^({numeral})[、，,．.]\s*")),
            Regex::new(&format!(r"^[（(]({numeral})[）)]\s*")),
            Regex::new(&format!(r"^({digits})([.．、])\s*")),
            Regex::new(&format!(r"^[（(]({digits})[）)]\s*")),
            // 「1)」：公文常见，Word 自动编号也常转成这个形状。
            Regex::new(&format!(r"^({digits})[)）]\s*")),
            Regex::new(r"^([①-⑳])\s*"),
        ]
        .map(|re| re.expect("valid numbering pattern"))
    });
    let prefix = if let Some(caps) = chapter.captures(text) {
        Prefix {
            level: 0,
            number: 0,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = chinese.captures(text) {
        Prefix {
            level: 1,
            number: 0,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = chinese_paren.captures(text) {
        Prefix {
            level: 2,
            number: 0,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = arabic.captures(text) {
        // 「1.5亿元」「3.2.1」不是编号。
        if text[caps[0].len()..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit() || ('０'..='９').contains(&ch))
            && caps[0].len() == caps[1].len() + caps[2].len()
        {
            return None;
        }
        Prefix {
            level: if from_list && &caps[2] == "、" { 1 } else { 3 },
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = arabic_paren.captures(text) {
        Prefix {
            level: 4,
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: from_list,
        }
    } else if let Some(caps) = arabic_stop.captures(text) {
        Prefix {
            level: 4,
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else {
        let caps = circled.captures(text)?;
        let ch = caps[1].chars().next()?;
        Prefix {
            level: 5,
            number: ch as u32 - '①' as u32 + 1,
            len: caps[0].len(),
            ambiguous: false,
        }
    };
    let rest = &text[prefix.len..];
    if rest.trim().is_empty() || is_enumeration(rest) {
        return None;
    }
    Some(prefix)
}

/// 把编号里的数字读成 `u32`：全角数字（`１`）与半角数字都认。
fn parse_number(text: &str) -> Option<u32> {
    let mut value = 0u32;
    for ch in text.chars() {
        let digit = match ch {
            '0'..='9' => ch as u32 - '0' as u32,
            '０'..='９' => ch as u32 - '０' as u32,
            _ => return None,
        };
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

/// 「一、二季度」「2、3月份」：顿号连着的是并列的数，不是编号。
fn is_enumeration(rest: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:[一二三四五六七八九十]+|[0-9０-９]+)[、，,季月年日号]")
            .expect("valid enumeration pattern")
    })
    .is_match(rest)
}
