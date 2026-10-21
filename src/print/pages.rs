//! 页码范围：把用户填的「1-3,5」解析成升序、不重叠的闭区间，再按份数与逐份
//! 打印展开成出纸顺序。

/// 闭区间，页码从 1 起。
pub(crate) type PageSpan = (u32, u32);

/// [`parse_ranges`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRanges {
    /// 升序、互不重叠也不相邻的区间。
    pub(crate) spans: Vec<PageSpan>,
    /// 有区间越过末页，已截到末页。不是错误，界面据此提示一句。
    pub(crate) truncated: bool,
}

/// 解析页码范围。`page_count` 为 0 表示页数未知（PDF 还没打开完），这时不做
/// 越界检查，也不接受「5-」这种开到末页的写法。
///
/// 分隔符认半角 / 全角逗号、顿号、分号与空白；范围符认半角 / 全角连字符、
/// 破折号与波浪线；数字认全角。用户从别处粘过来的页码这样都能用。
pub(crate) fn parse_ranges(input: &str, page_count: u32) -> Result<ParsedRanges, String> {
    let normalized: String = input
        .chars()
        .map(|ch| match ch {
            '０'..='９' => char::from_digit(ch as u32 - '０' as u32, 10).unwrap_or(ch),
            '，' | '、' | ';' | '；' => ',',
            '－' | '–' | '—' | '~' | '～' => '-',
            ch if ch.is_whitespace() => ',',
            ch => ch,
        })
        .collect();
    let mut spans = Vec::new();
    let mut truncated = false;
    for token in normalized.split(',').filter(|token| !token.is_empty()) {
        let (start, end) = match token.split_once('-') {
            Some((left, right)) => {
                let start = if left.is_empty() {
                    1
                } else {
                    parse_page(left, token)?
                };
                let end = if right.is_empty() {
                    if page_count == 0 {
                        return Err(format!("「{token}」缺少止页"));
                    }
                    page_count
                } else {
                    parse_page(right, token)?
                };
                (start, end)
            }
            None => {
                let page = parse_page(token, token)?;
                (page, page)
            }
        };
        if start > end {
            return Err(format!("「{token}」起页大于止页"));
        }
        if page_count > 0 {
            if start > page_count {
                return Err(format!("第 {start} 页不存在（共 {page_count} 页）"));
            }
            if end > page_count {
                truncated = true;
            }
            spans.push((start, end.min(page_count)));
        } else {
            spans.push((start, end));
        }
    }
    if spans.is_empty() {
        return Err("请填写页码，例如 1-3,5".to_string());
    }
    Ok(ParsedRanges {
        spans: merge(spans),
        truncated,
    })
}

fn parse_page(text: &str, token: &str) -> Result<u32, String> {
    let page: u32 = text
        .parse()
        .map_err(|_| format!("「{token}」不是有效的页码"))?;
    if page == 0 {
        return Err("页码从 1 开始".to_string());
    }
    Ok(page)
}

/// 排序并合并重叠或相邻的区间：「5,1-3,4」→ 1-5。
fn merge(mut spans: Vec<PageSpan>) -> Vec<PageSpan> {
    spans.sort_unstable();
    let mut merged: Vec<PageSpan> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1.saturating_add(1) => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// 区间写回成 CUPS `lp -P` 认的格式：「1-3,5」。
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn format_spans(spans: &[PageSpan]) -> String {
    spans
        .iter()
        .map(|&(start, end)| {
            if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// 区间里一共多少页。
pub(crate) fn span_page_count(spans: &[PageSpan]) -> u64 {
    spans
        .iter()
        .map(|&(start, end)| u64::from(end - start) + 1)
        .sum()
}

/// 出纸顺序（页码从 1 起）。逐份打印是「1 2 3, 1 2 3」，不逐份是「1 1, 2 2, 3 3」。
/// 份数与页数相乘可能很大，所以返回迭代器，不展开成数组。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn print_sequence(
    spans: &[PageSpan],
    copies: u32,
    collate: bool,
) -> impl Iterator<Item = u32> + '_ {
    let copies = copies.max(1);
    let pages = move || spans.iter().flat_map(|&(start, end)| start..=end);
    let collated = collate
        .then(move || (0..copies).flat_map(move |_| pages()))
        .into_iter()
        .flatten();
    let grouped = (!collate)
        .then(move || pages().flat_map(move |page| std::iter::repeat_n(page, copies as usize)))
        .into_iter()
        .flatten();
    collated.chain(grouped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(input: &str, count: u32) -> Vec<PageSpan> {
        parse_ranges(input, count).expect("应能解析").spans
    }

    #[test]
    fn parses_single_pages_and_ranges() {
        assert_eq!(spans("1-3,5", 10), vec![(1, 3), (5, 5)]);
        assert_eq!(spans("7", 10), vec![(7, 7)]);
    }

    #[test]
    fn accepts_chinese_punctuation_full_width_digits_and_spaces() {
        assert_eq!(spans("１－３，５、 8～9", 10), vec![(1, 3), (5, 5), (8, 9)]);
        assert_eq!(spans(" 2 4 ", 10), vec![(2, 2), (4, 4)]);
    }

    #[test]
    fn open_ended_ranges_run_to_the_edges() {
        assert_eq!(spans("8-", 10), vec![(8, 10)]);
        assert_eq!(spans("-2", 10), vec![(1, 2)]);
        // 页数未知时不知道末页在哪，开到末页的写法要报错。
        assert!(parse_ranges("8-", 0).is_err());
    }

    #[test]
    fn sorts_and_merges_overlapping_or_adjacent_spans() {
        assert_eq!(spans("5,1-3,4", 10), vec![(1, 5)]);
        assert_eq!(spans("2-6,3-4,9", 10), vec![(2, 6), (9, 9)]);
    }

    #[test]
    fn truncates_past_the_last_page_but_rejects_pages_that_do_not_exist() {
        let parsed = parse_ranges("3-99", 10).expect("应能解析");
        assert_eq!(parsed.spans, vec![(3, 10)]);
        assert!(parsed.truncated);
        assert!(parse_ranges("11", 10).is_err());
        // 页数未知时原样保留。
        assert_eq!(spans("3-99", 0), vec![(3, 99)]);
    }

    #[test]
    fn rejects_bad_input() {
        for input in ["", " ，", "0", "5-3", "a", "1-2-3", "1.5"] {
            assert!(parse_ranges(input, 10).is_err(), "「{input}」应当报错");
        }
    }

    #[test]
    fn formats_spans_for_lp() {
        assert_eq!(format_spans(&[(1, 3), (5, 5)]), "1-3,5");
        assert_eq!(span_page_count(&[(1, 3), (5, 5)]), 4);
    }

    #[test]
    fn print_sequence_follows_collate() {
        let spans = [(1, 2), (4, 4)];
        let collated: Vec<_> = print_sequence(&spans, 2, true).collect();
        assert_eq!(collated, vec![1, 2, 4, 1, 2, 4]);
        let grouped: Vec<_> = print_sequence(&spans, 2, false).collect();
        assert_eq!(grouped, vec![1, 1, 2, 2, 4, 4]);
        // 份数 0 按 1 份算。
        assert_eq!(print_sequence(&spans, 0, true).count(), 3);
    }
}
