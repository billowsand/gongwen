//! 公文引用：正文里的 `《名称》（发文字号）` 就是引用本身，不另存标记或元数据。
//!
//! 识别是确定性的：书名号后紧跟的括号里规范化后是「代字〔年份〕序号号」，就认作公文
//! 引用；`（试行）`、`（征求意见稿）` 之类不算。没有文号的文件只有名称，单凭书名号分不出
//! 是公文还是书名、法规，只有名称与稿件库或公文登记簿里某份无文号文件完全一致才认。
//! 来源关联在运行时按文号与名称比对（见 `draft_page::references`），正文里不存任何 ID。

use anyhow::{Result, ensure};
use std::{borrow::Cow, collections::BTreeMap, ops::Range};

/// 正文里识别出的一处公文引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Citation {
    /// 从 `《` 到收尾的 `）`（无文号时到 `》`）的字节范围。
    pub range: Range<usize>,
    pub title: String,
    /// 括号里的原文；无文号为空。
    pub number: String,
    /// 规范后的文号；无文号为空。
    pub normalized: String,
}

impl Citation {
    /// 规范写法：全角括号、六角括号年份。
    pub fn text(&self) -> String {
        display(&self.title, &self.normalized)
    }

    /// 正文里的写法是否已经规范。
    pub fn standard(&self, markdown: &str) -> bool {
        markdown.get(self.range.clone()) == Some(self.text().as_str())
    }
}

/// `《名称》（文号）`；没有文号只留书名号。
pub(crate) fn display(title: &str, number: &str) -> String {
    if number.is_empty() {
        format!("《{title}》")
    } else {
        format!("《{title}》（{number}）")
    }
}

/// 名称去掉用户顺手带上的外层书名号与首尾空白；名称里套的书名号按规范改成〈〉。
pub(crate) fn clean_title(title: &str) -> String {
    title
        .trim()
        .trim_start_matches('《')
        .trim_end_matches('》')
        .trim()
        .replace('《', "〈")
        .replace('》', "〉")
}

/// 登记、插入前的校验：名称不能空、不能跨行，有文号就得认得出是文号。
/// 返回清理后的名称与规范后的文号（无文号为空）。
pub(crate) fn validate(title: &str, number: &str, no_number: bool) -> Result<(String, String)> {
    let title = clean_title(title);
    ensure!(!title.is_empty(), "请填写公文名称");
    ensure!(!title.chars().any(char::is_control), "公文名称不能包含换行");
    if no_number {
        return Ok((title, String::new()));
    }
    let number = normalize_number(number);
    ensure!(
        !number.is_empty(),
        "请填写发文字号，或勾选该文件没有发文字号"
    );
    ensure!(
        is_document_number(&number),
        "认不出发文字号，应形如 某办函〔2026〕12号"
    );
    Ok((title, number))
}

/// 规范后的文号是否形如「代字〔四位年份〕序号号」。
pub(crate) fn is_document_number(normalized: &str) -> bool {
    let Some((code, rest)) = normalized.split_once('〔') else {
        return false;
    };
    let Some((year, serial)) = rest.split_once('〕') else {
        return false;
    };
    let code_len = code.chars().count();
    (1..=20).contains(&code_len)
        && !code
            .chars()
            .any(|ch| ch.is_whitespace() || "（）()《》，。；：".contains(ch))
        && year.len() == 4
        && year.chars().all(|ch| ch.is_ascii_digit())
        && serial.strip_suffix('号').is_some_and(|digits| {
            !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
        })
}

/// 只认有文号的引用。不查稿件库的地方（校对、AI 闸门）用它。
pub(crate) fn detect_numbered(markdown: &str) -> Vec<Citation> {
    detect(markdown, |_| false)
}

/// 识别正文里的公文引用。`untitled` 判断一个名称是不是已知的无文号文件。
/// 代码围栏、行内代码、公式与转义字符里的书名号不算。
pub(crate) fn detect(markdown: &str, untitled: impl Fn(&str) -> bool) -> Vec<Citation> {
    let mut found = Vec::new();
    for segment in plain_segments(markdown) {
        let text = &markdown[segment.clone()];
        let mut search = 0;
        while let Some(open) = text[search..].find('《').map(|at| search + at) {
            let title_start = open + '《'.len_utf8();
            let Some(close) = text[title_start..].find('》').map(|at| title_start + at) else {
                break;
            };
            let title = &text[title_start..close];
            // 中间又出现《：外层没闭合，从里层重新找。
            if let Some(inner) = title.rfind('《') {
                search = title_start + inner;
                continue;
            }
            let after = close + '》'.len_utf8();
            search = after;
            if title.trim().is_empty() || title.chars().count() > 100 {
                continue;
            }
            let numbered = parenthesized(&text[after..]).and_then(|(inner, length)| {
                let normalized = normalize_number(inner);
                is_document_number(&normalized).then(|| (after + length, inner, normalized))
            });
            if let Some((end, number, normalized)) = numbered {
                found.push(Citation {
                    range: segment.start + open..segment.start + end,
                    title: title.to_owned(),
                    number: number.to_owned(),
                    normalized,
                });
                search = end;
            } else if untitled(title) {
                found.push(Citation {
                    range: segment.start + open..segment.start + after,
                    title: title.to_owned(),
                    number: String::new(),
                    normalized: String::new(),
                });
            }
        }
    }
    found
}

/// 紧跟在书名号后的一对括号（全角半角混用也算，可嵌套）：返回括号内文字与整对括号的字节长度。
fn parenthesized(text: &str) -> Option<(&str, usize)> {
    if !text.starts_with(['（', '(']) {
        return None;
    }
    let mut depth = 0usize;
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        match ch {
            '（' | '(' => {
                if depth == 0 {
                    start = index + ch.len_utf8();
                }
                depth += 1;
            }
            '）' | ')' => {
                depth -= 1;
                if depth == 0 {
                    let inner = &text[start..index];
                    return (inner.chars().count() <= 40).then_some((inner, index + ch.len_utf8()));
                }
            }
            '\n' => return None,
            _ => {}
        }
        if index > 160 {
            return None;
        }
    }
    None
}

/// 正文里可以出现引用的片段：跳过代码围栏、行内代码、公式和反斜杠转义，按行切开。
fn plain_segments(markdown: &str) -> Vec<Range<usize>> {
    let mut segments = Vec::new();
    let mut offset = 0;
    let mut fence = None;
    let mut math: Option<usize> = None;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !fence_line(trimmed, &mut fence) && fence.is_none() {
            let mut index = 0;
            let mut start = 0;
            let mut code: Option<usize> = None;
            let mut cut = |from: usize, to: usize| {
                if from < to {
                    segments.push(offset + from..offset + to);
                }
            };
            while index < line.len() {
                let rest = &line[index..];
                let ch = rest.chars().next().unwrap();
                let plain = code.is_none() && math.is_none();
                if ch == '\\' {
                    if plain {
                        cut(start, index);
                    }
                    index += 1;
                    if index < line.len() {
                        index += line[index..].chars().next().unwrap().len_utf8();
                    }
                    if plain {
                        start = index;
                    }
                    continue;
                }
                let state = match ch {
                    '`' if math.is_none() => &mut code,
                    '$' if code.is_none() => &mut math,
                    _ => {
                        index += ch.len_utf8();
                        continue;
                    }
                };
                let length = rest.chars().take_while(|next| *next == ch).count();
                if plain {
                    cut(start, index);
                }
                if *state == Some(length) {
                    *state = None;
                } else if state.is_none() {
                    *state = Some(length);
                }
                index += length;
                if code.is_none() && math.is_none() {
                    start = index;
                }
            }
            if code.is_none() && math.is_none() {
                cut(start, line.len());
            }
        }
        if math == Some(1) {
            math = None;
        }
        offset += line.len();
    }
    segments
}

fn fence_line(line: &str, fence: &mut Option<(char, usize)>) -> bool {
    let Some(ch @ ('`' | '~')) = line.chars().next() else {
        return false;
    };
    let length = line.chars().take_while(|next| *next == ch).count();
    if length < 3 {
        return false;
    }
    if let Some((opening, minimum)) = *fence {
        if opening == ch && length >= minimum && line[length..].trim().is_empty() {
            *fence = None;
        }
    } else {
        *fence = Some((ch, length));
    }
    true
}

/// 文字校对屏蔽公文引用：引用的名称与文号是事实，不能被词表或规则改写。
/// 字节长度保持不变，其他文字的定位不漂移。
pub(crate) fn masked(markdown: &str) -> Cow<'_, str> {
    let citations = detect_numbered(markdown);
    if citations.is_empty() {
        return Cow::Borrowed(markdown);
    }
    let mut bytes = markdown.as_bytes().to_vec();
    for citation in citations {
        bytes[citation.range].fill(b' ');
    }
    Cow::Owned(String::from_utf8(bytes).expect("只把完整 UTF-8 范围换成空格"))
}

/// AI 与规则修订可以挪动引用、改周围的话，但不能改动、增删有文号的引用；
/// 只把括号写法改规范不算改动。
pub(crate) fn ensure_preserved(before: &str, after: &str) -> Result<()> {
    let counts = |text: &str| {
        let mut counts = BTreeMap::<String, usize>::new();
        for citation in detect_numbered(text) {
            *counts.entry(citation.text()).or_default() += 1;
        }
        counts
    };
    ensure!(
        counts(before) == counts(after),
        "该建议改变了公文引用的名称或文号，已放弃采纳，当前正文未改变"
    );
    Ok(())
}

/// 把手工输入的发文字号规范成「机关代字〔年份〕序号号」。
///
/// 六角括号不好打，登记时允许用方括号、圆括号、方头括号或干脆不写括号，例如
/// `某办函[2026]12号`、`某办函(2026)12`、`某办函2026 12`、`某办函2026年第012号`，
/// 都规范为 `某办函〔2026〕12号`。全角数字转半角，空白去掉，序号按 GB/T 9704 不编虚位。
/// 认不出年份的原样返回（只去空白），交给用户自己核对预览。
pub(crate) fn normalize_number(input: &str) -> String {
    const OPEN: &[char] = &['[', '(', '（', '【', '［', '〔', '{', '｛', '〖', '<', '＜'];
    const CLOSE: &[char] = &[']', ')', '）', '】', '］', '〕', '}', '｝', '〗', '>', '＞'];
    // 空白先当分隔符保留（`某办函 2026 12` 要靠它把年份和序号分开），最后统一去掉。
    const SEPARATORS: &[char] = &[' ', '-', '－', '_', '.', '．', '、', '/', '／', '·', '•'];
    let text: String = input
        .chars()
        .map(|ch| match ch {
            '０'..='９' => char::from(b'0' + (ch as u32 - '０' as u32) as u8),
            ch if ch.is_whitespace() => ' ',
            _ => ch,
        })
        .collect();
    let squeeze = |text: &str| text.chars().filter(|ch| *ch != ' ').collect::<String>();
    let chars: Vec<char> = text.chars().collect();
    // 第一段恰好四位、以 19 / 20 开头的数字当年份；代字里偶有数字也不会误认。
    let year_at = (0..chars.len().saturating_sub(3)).find(|&start| {
        chars[start..start + 4].iter().all(char::is_ascii_digit)
            && matches!(chars[start..start + 2], ['1', '9'] | ['2', '0'])
            && (start == 0 || !chars[start - 1].is_ascii_digit())
            && chars.get(start + 4).is_none_or(|ch| !ch.is_ascii_digit())
    });
    let Some(start) = year_at else {
        return squeeze(&text);
    };
    let prefix: String = chars[..start].iter().collect();
    let prefix =
        squeeze(prefix.trim_end_matches(|ch| OPEN.contains(&ch) || SEPARATORS.contains(&ch)));
    if prefix.is_empty() {
        return squeeze(&text);
    }
    let year: String = chars[start..start + 4].iter().collect();
    let rest: String = chars[start + 4..].iter().collect();
    let rest = squeeze(
        rest.trim_start_matches(|ch| CLOSE.contains(&ch) || SEPARATORS.contains(&ch) || ch == '年')
            .trim_start_matches('第'),
    );
    let serial = rest.strip_suffix('号').unwrap_or(&rest);
    let rest = if !serial.is_empty() && serial.chars().all(|ch| ch.is_ascii_digit()) {
        let trimmed = serial.trim_start_matches('0');
        format!("{}号", if trimmed.is_empty() { "0" } else { trimmed })
    } else {
        rest.clone()
    };
    format!("{prefix}〔{year}〕{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(markdown: &str) -> Vec<String> {
        detect_numbered(markdown)
            .into_iter()
            .map(|citation| markdown[citation.range].to_owned())
            .collect()
    }

    #[test]
    fn numbered_citations_are_detected_and_others_are_not() {
        let text = "根据《建设实施方案》（项办函〔2026〕56号）和《某某条例》（试行），\
                    参照《关于做好防火工作的通知》(某应急[2026]8号)及《会议纪要》（2026年版）。";
        assert_eq!(
            texts(text),
            [
                "《建设实施方案》（项办函〔2026〕56号）",
                "《关于做好防火工作的通知》(某应急[2026]8号)"
            ]
        );
        let citations = detect_numbered(text);
        assert!(citations[0].standard(text));
        assert!(!citations[1].standard(text));
        assert_eq!(
            citations[1].text(),
            "《关于做好防火工作的通知》（某应急〔2026〕8号）"
        );
    }

    #[test]
    fn nested_brackets_titles_and_full_width_parentheses() {
        let text =
            "《关于印发〈实施办法〉的通知》（某办发（2026）3号）与《甲《乙》（某办函〔2026〕1号）";
        assert_eq!(
            texts(text),
            [
                "《关于印发〈实施办法〉的通知》（某办发（2026）3号）",
                "《乙》（某办函〔2026〕1号）"
            ]
        );
        assert_eq!(detect_numbered(text)[0].normalized, "某办发〔2026〕3号");
    }

    #[test]
    fn code_math_and_escapes_are_skipped() {
        let text = "`《甲》（某办函〔2026〕1号）` $《乙》（某办函〔2026〕2号）$ \\《丙》（某办函〔2026〕3号）\n\
                    ```\n《丁》（某办函〔2026〕4号）\n```\n前文《戊》（某办函〔2026〕5号）";
        assert_eq!(texts(text), ["《戊》（某办函〔2026〕5号）"]);
    }

    #[test]
    fn untitled_citations_need_a_known_title() {
        let text = "报《关于报请审定的请示》，参阅《中华人民共和国保守国家秘密法》。";
        let found = detect(text, |title| title == "关于报请审定的请示");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text(), "《关于报请审定的请示》");
        assert!(found[0].number.is_empty());
    }

    #[test]
    fn validation_and_ai_preservation() {
        assert_eq!(
            validate("《某函》", "某办函[2026]12", false).unwrap(),
            ("某函".into(), "某办函〔2026〕12号".into())
        );
        assert_eq!(
            validate("关于印发《实施办法》的通知", "", true).unwrap().0,
            "关于印发〈实施办法〉的通知"
        );
        assert!(validate("某函", "", false).is_err());
        assert!(validate("某函", "第十二号", false).is_err());
        assert_eq!(validate("某函", "随便", true).unwrap().1, "");

        let before = "按《甲》(某办函[2026]1号)办理。";
        ensure_preserved(before, "请按《甲》（某办函〔2026〕1号）抓紧办理。").unwrap();
        assert!(ensure_preserved(before, "按《甲》（某办函〔2026〕2号）办理。").is_err());
        assert!(ensure_preserved(before, "按要求办理。").is_err());
        let masked = masked(before);
        assert_eq!(masked.len(), before.len());
        assert!(!masked.contains('甲'));
    }

    #[test]
    fn document_number_input_is_normalized_to_hexagonal_brackets() {
        for input in [
            "某办函〔2026〕12号",
            "某办函[2026]12号",
            "某办函(2026)12",
            "某办函（2026）12号",
            "某办函【2026】12号",
            "某办函 2026 12",
            "某办函2026-12",
            "某办函2026年第012号",
            "某办函 [2026] 12 号",
            "某办函［２０２６］１２号",
        ] {
            assert_eq!(normalize_number(input), "某办函〔2026〕12号", "{input}");
        }
        // 认不出年份、或只有年份没有代字时原样（去空白）交给用户核对。
        assert_eq!(normalize_number("某办 函12号"), "某办函12号");
        assert_eq!(normalize_number("2026-12"), "2026-12");
        // 序号后的附加文字不动，只统一括号。
        assert_eq!(
            normalize_number("某办函[2026]12号附件"),
            "某办函〔2026〕12号附件"
        );
        // 代字本身带数字时，只认四位年份。
        assert_eq!(normalize_number("某1办函(2026)3"), "某1办函〔2026〕3号");
        assert_eq!(normalize_number(""), "");
    }
}
