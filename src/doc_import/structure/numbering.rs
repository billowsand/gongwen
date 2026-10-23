//! 段首编号的识别与编号方案判定。
//!
//! 编号样式（`第X章`、`一、`、`（一）`、`1.`、`（1）`、`①`，以及第二期加的点分、罗马
//! 数字、字母）从浅到深定层级；层级只决定「大概多深」，最终排成几级由 `structure` 的
//! `heading_level_map` 按全篇实际用到的层级压紧。
//!
//! **编号方案看整段文字定，不看单行**：粘一段研究报告，`1`、`1.1`、`1.1.1` 要按
//! `##`、`###`、`####` 排；粘一段讲话稿，孤零零一句「3.2.1 技术路线」不是编号。
//! 方案判定见 [`Numbering::detect`]，逐行识别见 [`parse_prefix`]。

use regex::Regex;
use std::sync::OnceLock;

use super::{TITLE_MAX, chars, markdown_heading, strip_bold};

// ---------------------------------------------------------------- 编号

pub(super) struct Prefix {
    pub(super) level: u8,
    pub(super) number: u32,
    /// 编号（连同其后空白）占的字节数。
    pub(super) len: usize,
    pub(super) ambiguous: bool,
    /// 点分编号的完整前缀（`1.1` → `[1, 1]`），其它编号方案为空。
    pub(super) dotted: Option<Vec<u32>>,
}

// ---------------------------------------------------------------- 编号方案

/// 这一段文字用的是哪套编号方案。段首编号的定级跟它走：`1.` 在「一、（一）1.」的
/// 老方案里是第三层，在点分方案（`1.1`、`1.2`）里是最浅的一层。
///
/// 方案看整段文字定，不看单行：粘一段研究报告，`1`、`1.1`、`1.1.1` 要按 ##、###、
/// #### 排；粘一段讲话稿，孤零零一句「3.2.1 技术路线」不是编号。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Numbering {
    /// 点分方案：至少两个点分编号，而且前后衔接（`1.1`、`1.2`、`2.1`）。
    dotted: bool,
    /// 连续的 `I.`、`II.`：ASCII 罗马数字算编号（单独一个 `I.` 不算，怕是英文句子）。
    roman: bool,
    /// 连续的 `A.`、`B.`：字母编号算编号。
    letters: bool,
}

impl Numbering {
    /// 扫一遍原文决定方案。只认行首、且像编号的行（见 [`dotted_candidate`]、
    /// [`roman_candidate`]、[`letter_candidate`]）。
    pub(super) fn detect(markdown: &str) -> Self {
        let mut dotted: Vec<Vec<u32>> = Vec::new();
        let mut roman: Vec<u32> = Vec::new();
        let mut letters: Vec<u32> = Vec::new();
        for line in markdown.lines() {
            let trimmed = line.trim();
            // 标题行（`## 1.1 背景`）的编号在 `#` 后面，要剥掉记号再看。
            let text = markdown_heading(trimmed).map_or(trimmed, |(_, text)| text);
            if let Some(parts) = dotted_candidate(text) {
                dotted.push(parts);
            }
            if let Some(value) = roman_candidate(text) {
                roman.push(value);
            }
            if let Some(value) = letter_candidate(text) {
                letters.push(value);
            }
        }
        Self {
            dotted: dotted_scheme(&dotted),
            roman: numbering_in_a_row(&roman),
            letters: numbering_in_a_row(&letters),
        }
    }
}

/// 点分编号是不是成体系的：至少两处，而且有相邻两处衔接得上。衔接有三种：
/// 同一父级往后走（`1.1`→`1.2`，中间跳号也算——那正是要提醒的）、下一层
/// （`1.1`→`1.1.1`）、父级进位、子级从头（`1.2`→`2.1`）。
fn dotted_scheme(items: &[Vec<u32>]) -> bool {
    items.len() >= 2 && items.windows(2).any(|pair| coherent(&pair[0], &pair[1]))
}

fn coherent(previous: &[u32], next: &[u32]) -> bool {
    let depth = previous.len().min(next.len());
    if depth == 0 {
        return false;
    }
    // 共享前缀（父级）相同，并且往后走：`1.2`→`1.3`，跳号的 `1.2`→`1.4` 也算
    // （跳号由 `report_number_jumps` 单独报）。
    if previous[..depth - 1] == next[..depth - 1] && next[depth - 1] > previous[depth - 1] {
        return true;
    }
    // 下一项是上一项的子级：`1.1`→`1.1.1`。
    if next.len() > previous.len() && next[..previous.len()] == previous[..] {
        return true;
    }
    // 父级进位、子级从头：`1.2`→`2.1`。
    next.len() == previous.len() && next[0] == previous[0] + 1 && next[next.len() - 1] == 1
}

/// 一组编号里有没有「连续的下一号」。罗马数字与字母都要成组出现才认。
fn numbering_in_a_row(values: &[u32]) -> bool {
    values.len() >= 2 && values.windows(2).any(|pair| pair[1] == pair[0] + 1)
}

/// 一行像不像点分编号的开头：`1.1 标题`、`1.1.标题`、`1.1标题`。
///
/// 排除小数量词（`1.5亿元`、`1.1倍`）、超过标题长度的整行，以及跟在数字后面的
/// 小数（后面的数字由正则一起吃掉，剩下以数字或点开头的就不认）。
fn dotted_candidate(line: &str) -> Option<Vec<u32>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^([0-9０-９]{1,3}(?:[.．][0-9０-９]{1,3}){1,3})")
            .expect("valid dotted pattern")
    });
    let caps = re.captures(line)?;
    if chars(line) > TITLE_MAX {
        return None;
    }
    let parts = caps[1]
        .split(['.', '．'])
        .map(parse_number)
        .collect::<Option<Vec<u32>>>()?;
    if parts.len() < 2 || parts.contains(&0) {
        return None;
    }
    let end = skip_after_marker(line, caps[0].len());
    let rest = &line[end..];
    let first = rest.chars().next()?;
    // 编号后面要跟空格（刚吃掉）或直接跟汉字 / 西文字母；跟数字、点、标点都不是编号。
    if !(is_han(first) || first.is_ascii_alphabetic()) || starts_with_unit(rest) {
        return None;
    }
    Some(parts)
}

/// 一行 `I. 标题` / `II、标题` 的罗马序号。
fn roman_candidate(line: &str) -> Option<u32> {
    let (value, end) = ascii_roman_run(line)?;
    let rest = &line[end..];
    if !matches!(
        rest.chars().next()?,
        '、' | '．' | '.' | ',' | '，' | ')' | '）'
    ) {
        return None;
    }
    (chars(line) <= TITLE_MAX).then_some(value)
}

/// 一行 `A. 标题` / `（A）标题` 的字母序号。
fn letter_candidate(line: &str) -> Option<u32> {
    let (value, end) = ascii_letter_prefix(line)?;
    (chars(line) <= TITLE_MAX && end > 0).then_some(value)
}

/// 编号后面紧跟这些字就是「数量 + 单位」，不是编号。
///
/// 只收「不会当标题开头」的单位（国标计量单位的常见写法与货币、百分比符号）：用来
/// 排除 `1.1倍`、`2.3万元`、`3.5%` 这类小数。个、项、家、人、分、时、年、月这些
/// 单字量词故意不收——它们同时是常见词的第一个字（分析、项目、家长、人员、时间……），
/// 收进来会把「2.2分析框架」这种正经编号标题判成数量。
const NUMBER_UNITS: &[&str] = &[
    "亿元",
    "万元",
    "亿",
    "万",
    "元",
    "倍",
    "％",
    "%",
    "‰",
    "千米",
    "公里",
    "厘米",
    "毫米",
    "平方米",
    "平方千米",
    "平方公里",
    "立方米",
    "千克",
    "公斤",
    "克",
    "吨",
    "毫升",
    "升",
    "米",
    "亩",
    "公顷",
];

fn starts_with_unit(text: &str) -> bool {
    NUMBER_UNITS.iter().any(|unit| text.starts_with(unit))
}

/// 编号（已经占 `end` 字节）后面的分隔符与空白一起吃掉，返回前缀的总长度。
fn skip_after_marker(text: &str, end: usize) -> usize {
    let mut end = end;
    if let Some(ch) = text[end..].chars().next()
        && matches!(ch, '、' | '，' | ',' | '．' | '.' | '：' | ':' | '）' | ')')
    {
        end += ch.len_utf8();
    }
    while let Some(ch) = text[end..].chars().next() {
        if ch.is_whitespace() {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    end
}

/// 认段首编号。`from_list` 表示编号来自 Word 自动编号：汉字计数的「一、」会被
/// 转成「1、」，所以这时 `N、` 算一级标题。`numbering` 是整段文字定下来的编号方案。
pub(super) fn parse_prefix(text: &str, from_list: bool, numbering: Numbering) -> Option<Prefix> {
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
            number: numeral_value(&caps[1]).unwrap_or(0),
            len: caps[0].len(),
            ambiguous: false,
            dotted: None,
        }
    } else if let Some(prefix) = roman_prefix(text, numbering) {
        prefix
    } else if let Some(caps) = chinese.captures(text) {
        Prefix {
            level: 1,
            number: numeral_value(&caps[1]).unwrap_or(0),
            len: caps[0].len(),
            ambiguous: false,
            dotted: None,
        }
    } else if let Some(caps) = chinese_paren.captures(text) {
        Prefix {
            level: 2,
            number: numeral_value(&caps[1]).unwrap_or(0),
            len: caps[0].len(),
            ambiguous: false,
            dotted: None,
        }
    } else if numbering.dotted
        && let Some(prefix) = dotted_prefix(text)
    {
        prefix
    } else if numbering.dotted
        && let Some(prefix) = bare_number_prefix(text)
    {
        prefix
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
            level: if numbering.dotted || (from_list && &caps[2] == "、") {
                1
            } else {
                3
            },
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: false,
            dotted: None,
        }
    } else if numbering.letters
        && let Some(prefix) = letter_prefix(text)
    {
        prefix
    } else if let Some(caps) = arabic_paren.captures(text) {
        Prefix {
            level: 4,
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: from_list,
            dotted: None,
        }
    } else if let Some(caps) = arabic_stop.captures(text) {
        Prefix {
            level: 4,
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: false,
            dotted: None,
        }
    } else {
        let caps = circled.captures(text)?;
        let ch = caps[1].chars().next()?;
        Prefix {
            level: 5,
            number: ch as u32 - '①' as u32 + 1,
            len: caps[0].len(),
            ambiguous: false,
            dotted: None,
        }
    };
    let rest = &text[prefix.len..];
    if rest.trim().is_empty() || is_enumeration(rest) {
        return None;
    }
    Some(prefix)
}

/// 罗马数字前缀：全角罗马数字（Ⅰ–Ⅻ）总是认；ASCII 的 `I.`、`II.` 只在整段文字
/// 里成组出现时认（见 [`Numbering::roman`]），免得把英文句子开头的 `I.` 当编号。
fn roman_prefix(text: &str, numbering: Numbering) -> Option<Prefix> {
    let mut values = Vec::new();
    let mut end = 0;
    for ch in text.chars() {
        let Some(value) = full_width_roman_value(ch) else {
            break;
        };
        values.push(value);
        end += ch.len_utf8();
    }
    if values.is_empty() {
        if !numbering.roman {
            return None;
        }
        let (value, len) = ascii_roman_run(text)?;
        return (len > 0).then(|| Prefix {
            level: 0,
            number: value,
            len: skip_after_marker(text, len),
            ambiguous: false,
            dotted: None,
        });
    }
    let value = roman_value(&values)?;
    let separator = text[end..].chars().next()?;
    if !matches!(separator, '、' | '．' | '.' | ',' | '，' | ')' | '）') {
        return None;
    }
    Some(Prefix {
        level: 0,
        number: value,
        len: skip_after_marker(text, end),
        ambiguous: false,
        dotted: None,
    })
}

/// 点分编号前缀（`1.1`、`1.1.1`）。第一段最浅，每多一段深一层，最多到 `#####`。
fn dotted_prefix(text: &str) -> Option<Prefix> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^([0-9０-９]{1,3}(?:[.．][0-9０-９]{1,3}){1,3})")
            .expect("valid dotted pattern")
    });
    let caps = re.captures(text)?;
    let parts = caps[1]
        .split(['.', '．'])
        .map(parse_number)
        .collect::<Option<Vec<u32>>>()?;
    if parts.len() < 2 || parts.contains(&0) {
        return None;
    }
    let len = skip_after_marker(text, caps[0].len());
    let rest = &text[len..];
    let first = rest.chars().next()?;
    if !(is_han(first) || first.is_ascii_alphabetic()) || starts_with_unit(rest) {
        return None;
    }
    Some(Prefix {
        level: (parts.len() as u8).min(4),
        number: *parts.last()?,
        len,
        ambiguous: false,
        dotted: Some(parts),
    })
}

/// 点分方案下不带点的顶层编号：`1 标题`、`2　标题`。老方案里这种写法不是编号。
fn bare_number_prefix(text: &str) -> Option<Prefix> {
    let mut end = 0;
    for ch in text.chars() {
        if ch.is_ascii_digit() || ('０'..='９').contains(&ch) {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 || end > 3 * 4 {
        return None;
    }
    let number = parse_number(&text[..end])?;
    let first = text[end..].chars().next()?;
    if !(first.is_whitespace() || is_han(first)) {
        return None;
    }
    let len = skip_after_marker(text, end);
    let rest = &text[len..];
    if rest.is_empty() || starts_with_unit(rest) {
        return None;
    }
    Some(Prefix {
        level: 1,
        number,
        len,
        ambiguous: false,
        dotted: None,
    })
}

/// 字母编号前缀：`A.`、`A、`、`（A）`、`(a)`。放在 `（1）` 之下一层。
fn letter_prefix(text: &str) -> Option<Prefix> {
    let (number, end) = ascii_letter_prefix(text)?;
    Some(Prefix {
        level: 6,
        number,
        len: skip_after_marker(text, end),
        ambiguous: false,
        dotted: None,
    })
}

/// `A.` / `（A）` 这样的字母编号：返回序号与编号占的字节数（不含后面的分隔与空白）。
fn ascii_letter_prefix(text: &str) -> Option<(u32, usize)> {
    let first = text.chars().next()?;
    let (letter, end) = if first == '（' || first == '(' {
        let close = text[first.len_utf8()..].chars().next()?;
        if !close.is_ascii_alphabetic() {
            return None;
        }
        let after = first.len_utf8() + close.len_utf8();
        let paren = text[after..].chars().next()?;
        if !matches!(paren, '）' | ')') {
            return None;
        }
        (close, after + paren.len_utf8())
    } else {
        if !first.is_ascii_alphabetic() {
            return None;
        }
        let separator = text[1..].chars().next()?;
        if !matches!(separator, '.' | '．' | '、' | ')' | '）') {
            return None;
        }
        (first, 1 + separator.len_utf8())
    };
    Some((letter.to_ascii_lowercase() as u32 - 'a' as u32 + 1, end))
}

/// 从行首读一段 ASCII 罗马数字（`I`、`II`、`XII`），返回数值与长度。
fn ascii_roman_run(text: &str) -> Option<(u32, usize)> {
    let mut values = Vec::new();
    let mut len = 0;
    for ch in text.chars() {
        let Some(value) = ascii_roman_value(ch) else {
            break;
        };
        values.push(value);
        len += ch.len_utf8();
    }
    if values.is_empty() || len > 7 {
        return None;
    }
    Some((roman_value(&values)?, len))
}

fn ascii_roman_value(ch: char) -> Option<u32> {
    Some(match ch {
        'I' => 1,
        'V' => 5,
        'X' => 10,
        'L' => 50,
        'C' => 100,
        'D' => 500,
        'M' => 1000,
        _ => return None,
    })
}

/// 全角罗马数字 Ⅰ–Ⅻ（大写）与 ⅰ–ⅻ（小写）的字面值。
fn full_width_roman_value(ch: char) -> Option<u32> {
    let code = ch as u32;
    let offset = match code {
        0x2160..=0x216F => code - 0x2160,
        0x2170..=0x217F => code - 0x2170,
        _ => return None,
    };
    Some(match offset {
        0 => 1,
        1 => 2,
        2 => 3,
        3 => 4,
        4 => 5,
        5 => 6,
        6 => 7,
        7 => 8,
        8 => 9,
        9 => 10,
        10 => 11,
        11 => 12,
        12 => 50,
        13 => 100,
        14 => 500,
        15 => 1000,
        _ => return None,
    })
}

/// 罗马数字按「小在大前是减」求和；认不出合法写法时返回 `None`。
fn roman_value(values: &[u32]) -> Option<u32> {
    let mut total = 0u32;
    for (index, &value) in values.iter().enumerate() {
        match values.get(index + 1) {
            Some(&next) if value < next => total = total.checked_sub(value)?,
            _ => total = total.checked_add(value)?,
        }
    }
    (1..=3999).contains(&total).then_some(total)
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

/// 汉字序号（一、十一、二十一、一百）或阿拉伯数字（含全角）读成 `u32`。
fn numeral_value(text: &str) -> Option<u32> {
    if text
        .chars()
        .all(|ch| ch.is_ascii_digit() || ('０'..='９').contains(&ch))
    {
        return parse_number(text);
    }
    chinese_number(text)
}

/// 汉字序号：一 … 九十九、一百。只认到千，够公文编号用了。
fn chinese_number(text: &str) -> Option<u32> {
    let mut total = 0u32;
    let mut current = 0u32;
    let mut any = false;
    for ch in text.chars() {
        match ch {
            '一'..='九' => {
                current = ch as u32 - '一' as u32 + 1;
                any = true;
            }
            '零' | '〇' => {
                current = 0;
                any = true;
            }
            '十' => {
                total += current.max(1) * 10;
                current = 0;
                any = true;
            }
            '百' => {
                total += current.max(1) * 100;
                current = 0;
                any = true;
            }
            _ => return None,
        }
    }
    any.then(|| total + current)
}

/// 汉字（含扩展 A 与兼容区）。
fn is_han(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
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
/// 编号方案：「这一行有没有编号」要看整段文字定下来的是哪套（点分编号只看单行
/// 认不出来）。粘贴时给样式标题平移层级用，见 `rich_paste::headings`。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Scheme(Numbering);

/// 按整段文字定编号方案。
pub(crate) fn marker_scheme(markdown: &str) -> Scheme {
    Scheme(Numbering::detect(markdown))
}

/// 这一行段首编号的层级；没有编号返回 `None`。
pub(crate) fn marker_level(scheme: Scheme, text: &str) -> Option<u8> {
    parse_prefix(&strip_bold(text), false, scheme.0).map(|prefix| prefix.level)
}
