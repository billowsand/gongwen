//! 会议纪要专有的排版规则：出席名单的排法与段内「一是……。」楷体。
//!
//! 只管文字层面的判定，PDF（Typst）、Word 与预览各自按这里的结果排。版式见
//! `docs/meeting-minutes-design.md` 第二节。

use std::ops::Range;

use crate::models::{DraftInput, split_units};
use crate::units::UnitDisplay;

/// 一格放得下的最多字数。网格每行 6 格，格宽是「版心 − 左空二字 − 三字标签」均分，
/// 三号字约 3.7 个字宽；超过 3 字（带单位的、四字以上姓名）就放不进。
const ROSTER_CELL_CHARS: usize = 3;

/// 「一是」楷体最多管多少字。再长多半是整句论述而不是概括句，只把序号排楷体。
const ENUMERATION_KAI_MAX_CHARS: usize = 40;

/// 纪要标志：表单填了就用，否则「发文单位全称＋会议纪要」。
pub(crate) fn minutes_heading(input: &DraftInput, display: &UnitDisplay) -> String {
    let explicit = input.minutes.heading.trim();
    if explicit.is_empty() {
        format!("{}会议纪要", display.full_name(&input.profile.issuing_unit))
    } else {
        explicit.to_string()
    }
}

/// 编号「〔2026〕3号」的纸面文字：没有机关代字，预览版序号留一字空。
pub(crate) fn minutes_number_text(input: &DraftInput) -> String {
    let serial = if crate::export::element_display::is_preview_version(input) {
        crate::export::element_display::PREVIEW_PLACEHOLDER.to_string()
    } else {
        input.profile.document_number.trim().to_string()
    };
    format!("〔{}〕{serial}号", input.document_year())
}

/// 名单的一行：出席 / 列席 / 请假。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RosterLine {
    pub label: &'static str,
    pub names: Vec<String>,
    /// 每行 6 格对齐排；为假时连排「甲、乙、丙」。
    pub grid: bool,
}

/// 名单各行，空项不出现。各行独立决定网格还是连排。
pub(crate) fn roster(input: &DraftInput) -> Vec<RosterLine> {
    [
        ("出席", input.attendees.as_str()),
        ("列席", input.minutes.nonvoting.as_str()),
        ("请假", input.minutes.absent.as_str()),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        let names = split_units(value);
        if names.is_empty() {
            return None;
        }
        let grid = names
            .iter()
            .all(|name| name.chars().count() <= ROSTER_CELL_CHARS);
        Some(RosterLine { label, names, grid })
    })
    .collect()
}

/// 段内「一是……。」要排楷体的字符范围（按字符计，不是字节）。
///
/// 规则：`一是`……`十是` 出现在段首或紧跟「。；：」之后，从序号起到下一个
/// 「。；！？」之前排楷体，标点本身不变；前一个字是别的汉字（「统一是」「唯一是」）
/// 不算。到段尾都没有这些标点，或这一截超过 40 字，就只把「X是」两字排楷体。
pub(crate) fn enumeration_kai_spans(text: &str) -> Vec<Range<usize>> {
    const NUMERALS: &str = "一二三四五六七八九十";
    const OPENERS: &str = "。；：";
    const CLOSERS: &str = "。；！？";
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut index = 0;
    while index + 1 < chars.len() {
        let starts = NUMERALS.contains(chars[index])
            && chars[index + 1] == '是'
            && (index == 0 || OPENERS.contains(chars[index - 1]));
        if !starts {
            index += 1;
            continue;
        }
        let end = chars[index + 2..]
            .iter()
            .position(|ch| CLOSERS.contains(*ch))
            .map(|offset| index + 2 + offset)
            .filter(|end| end - index <= ENUMERATION_KAI_MAX_CHARS)
            .unwrap_or(index + 2);
        spans.push(index..end);
        index = end;
    }
    spans
}

/// 把一段片段文字按楷体范围切开：`offset` 是这段在全段可见文字里的起点（按字符），
/// `spans` 来自 [`enumeration_kai_spans`]。返回依次相接的（文字，是否楷体）。
/// PDF、Word、预览各自的片段结构不同，切点都从这里取。
pub(crate) fn kai_pieces(text: &str, offset: usize, spans: &[Range<usize>]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (index, ch) in text.chars().enumerate() {
        let kai = spans.iter().any(|span| span.contains(&(offset + index)));
        match out.last_mut() {
            Some((piece, current)) if *current == kai => piece.push(ch),
            _ => out.push((ch.to_string(), kai)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kai_texts(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        enumeration_kai_spans(text)
            .into_iter()
            .map(|range| chars[range].iter().collect())
            .collect()
    }

    #[test]
    fn enumerations_after_full_stops_are_kai_up_to_the_stop() {
        assert_eq!(
            kai_texts("会议认为，工作有成效。一是统一思想。二是压实责任；三是强化保障！"),
            ["一是统一思想", "二是压实责任", "三是强化保障"]
        );
        assert_eq!(kai_texts("一是抓落实。"), ["一是抓落实"]);
    }

    #[test]
    fn enumerations_inside_words_are_left_alone() {
        assert!(kai_texts("全市统一是前提，唯一是底线。").is_empty());
        assert!(kai_texts("会议要求，一是加强领导。").is_empty());
    }

    #[test]
    fn long_or_unterminated_items_only_mark_the_numeral() {
        let long = format!("。一是{}。", "很".repeat(45));
        assert_eq!(kai_texts(&long), ["一是"]);
        assert_eq!(kai_texts("。二是没有句号"), ["二是"]);
    }

    #[test]
    fn number_has_no_department_code_and_blanks_serial_in_preview() {
        let mut input = DraftInput::default();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "3".into();
        input.profile.department_code = "某民函".into();
        assert_eq!(minutes_number_text(&input), "〔2026〕3号");
        input.profile.letter_version = crate::models::LetterVersion::Preview;
        assert_eq!(minutes_number_text(&input), "〔2026〕\u{2003}号");
    }

    #[test]
    fn heading_defaults_to_issuing_unit() {
        let display = UnitDisplay::new(&[]);
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "某某市民政局".into();
        assert_eq!(minutes_heading(&input, &display), "某某市民政局会议纪要");
        input.minutes.heading = "某某市民政局局长办公会议纪要".into();
        assert_eq!(
            minutes_heading(&input, &display),
            "某某市民政局局长办公会议纪要"
        );
    }

    #[test]
    fn kai_pieces_cut_at_span_edges_with_offset() {
        let spans = enumeration_kai_spans("好。一是统一。");
        // 片段「一是统」从全段第 2 个字开始。
        assert_eq!(
            kai_pieces("。一是统", 1, &spans),
            [("。".to_string(), false), ("一是统".to_string(), true)]
        );
        assert!(kai_pieces("", 0, &spans).is_empty());
    }

    #[test]
    fn roster_switches_to_running_text_per_line() {
        let mut input = DraftInput {
            attendees: "张三、李四、欧阳明月".into(),
            ..Default::default()
        };
        input.minutes.nonvoting = "王五、赵六".into();
        let rows = roster(&input);
        assert_eq!(rows.len(), 2, "请假为空不出现");
        assert_eq!(rows[0].label, "出席");
        assert!(!rows[0].grid, "有四字名就连排");
        assert_eq!(rows[1].label, "列席");
        assert!(rows[1].grid);
        assert_eq!(rows[1].names, ["王五", "赵六"]);
    }
}
