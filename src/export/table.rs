//! 与 mdx official/research 共用思路的智能表格列宽分析。

use super::{ColumnAlign, RedlineKind, TableSpan, inline_segments, redline_chunks, table_span_at};
use regex::Regex;
use std::ops::Range;
use std::sync::OnceLock;

const SHORT_TEXT_THRESHOLD: f64 = 8.0;
const LONG_TEXT_THRESHOLD: f64 = 20.0;
const NUMERIC_RATIO_THRESHOLD: f64 = 0.8;
const MIN_WIDTH_RATIO: f64 = 0.8;
const MAX_WIDTH_RATIO: f64 = 4.0;
const CJK_WIDTH_FACTOR: f64 = 1.8;
const NARROW_NUMERIC_MAX_DIGITS: usize = 2;
const NARROW_NUMERIC_WIDTH_EM: f64 = 2.0;
// 公函版心宽 156mm，附件表格为 14bp，折合约 31.6 个全角字宽。
// tabularray 默认每列左右各留约 6pt，因此另按每列 0.85em 计入横向开销。
const PORTRAIT_TABLE_WIDTH_EM: f64 = 31.6;
const COLUMN_PADDING_EM: f64 = 0.85;
const MIN_READABLE_COLUMN_EM: f64 = 2.0;
const MAX_PROSE_COLUMN_EM: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnAlignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ColumnWidth {
    Relative(f64),
    FixedEm(f64),
}

#[derive(Debug, Clone, Copy)]
struct ColumnLayout {
    alignment: ColumnAlignment,
    width: ColumnWidth,
}

#[derive(Debug, Clone, Default)]
struct ColumnStats {
    total_width: f64,
    max_width: f64,
    count: usize,
    numeric_count: usize,
    has_long_text: bool,
    has_punctuation: bool,
    all_short_digits: bool,
}

impl ColumnStats {
    /// 整列都是短数字的列按固定宽度处理：它既不参与相对宽度分配，
    /// 也不该被跨过它的合并单元格撑宽。
    fn is_fixed(&self) -> bool {
        self.count > 0 && self.all_short_digits
    }
}

fn display_width(value: &str) -> f64 {
    value
        .chars()
        .map(|ch| if ch.is_ascii() { 1.0 } else { CJK_WIDTH_FACTOR })
        .sum()
}

fn plain_cell_text(value: &str) -> String {
    // 滤掉花脸稿哨兵：它们不占版面，但字符数会被算进列宽，让一张表凭空变宽
    // 甚至被误判成需要横排。
    inline_segments(value)
        .iter()
        .flat_map(|segment| segment.text.chars())
        .filter(|ch| !crate::export::is_redline_sentinel(*ch))
        .collect()
}

fn cjk_em_width(value: &str) -> f64 {
    display_width(&plain_cell_text(value)) / CJK_WIDTH_FACTOR
}

/// TeX 通常不会在连续的西文、数字和常用连接符中间断行，这部分必须完整放进列内。
fn max_unbreakable_width_em(value: &str) -> f64 {
    let plain = plain_cell_text(value);
    let mut current = String::new();
    let mut maximum: f64 = 0.0;
    for ch in plain.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | ':' | '.' | '%' | '_' | '@') {
            current.push(ch);
        } else if !current.is_empty() {
            maximum = maximum.max(display_width(&current) / CJK_WIDTH_FACTOR);
            current.clear();
        }
    }
    maximum
}

fn percentile_75(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    // nearest-rank：ceil(0.75 * n) - 1，避免两三个样本时退化成取最短值。
    values[(values.len() * 3).div_ceil(4) - 1]
}

/// 判断一张表在竖向版心内是否会因列数和列内容共同作用而过度拥挤。
///
/// 算法估算每列保持可读性所需的最小宽度：短字段按一行、普通字段按两行、
/// 含句读的长说明按三行容纳；表头最多允许两行，并保护不可断开的西文/数字串。
/// 所有列的最小宽度加上单元格内边距超过竖向版心时，横页才真正有收益。
/// 横向合并单元格的需求按跨度摊到它跨过的各列上，合并标题不会单独撑大一列。
pub(super) fn requires_landscape(rows: &[Vec<String>], spans: &[TableSpan]) -> bool {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if column_count <= 1 {
        return false;
    }

    let required_content_width = (0..column_count)
        .map(|column_index| column_requirement(rows, spans, column_index))
        .sum::<f64>();

    required_content_width + column_count as f64 * COLUMN_PADDING_EM > PORTRAIT_TABLE_WIDTH_EM
}

/// 单列保持可读所需的最小字宽；横向合并格的需求摊分到跨度上后与普通内容取较大值。
fn column_requirement(rows: &[Vec<String>], spans: &[TableSpan], column_index: usize) -> f64 {
    // 横向合并格的内容不能算到单列头上，否则合并标题会被当成“一列要装下整句”
    // 而误判成需要横排。锚点格与覆盖格都跳过，需求另由 merged_requirement 给出。
    let single_column = |row: usize| {
        table_span_at(spans, row, column_index).is_none_or(|span| span.column_span == 1)
    };
    let header = rows
        .first()
        .filter(|_| single_column(0))
        .and_then(|row| row.get(column_index))
        .map_or("", String::as_str);
    let body = rows
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(row_index, _)| single_column(*row_index))
        .filter_map(|(_, row)| row.get(column_index))
        .filter(|cell| !cell.trim().is_empty())
        .collect::<Vec<_>>();

    let merged = merged_requirement(rows, spans, column_index);
    if !body.is_empty()
        && body
            .iter()
            .all(|cell| is_short_digits(&plain_cell_text(cell)))
    {
        return merged.max(NARROW_NUMERIC_WIDTH_EM);
    }

    let typical_width = percentile_75(body.iter().map(|cell| cjk_em_width(cell)).collect());
    let prose = body.iter().any(|cell| has_sentence_punctuation(cell))
        || typical_width > LONG_TEXT_THRESHOLD / CJK_WIDTH_FACTOR;
    let target_lines = if prose {
        3.0
    } else if typical_width > SHORT_TEXT_THRESHOLD / CJK_WIDTH_FACTOR {
        2.0
    } else {
        1.0
    };
    let body_width = (typical_width / target_lines).min(MAX_PROSE_COLUMN_EM);
    let header_width = cjk_em_width(header);
    let header_minimum = if header_width > 4.0 {
        header_width / 2.0
    } else {
        header_width
    };
    let unbreakable_width = std::iter::once(header)
        .chain(body.iter().map(|cell| cell.as_str()))
        .map(max_unbreakable_width_em)
        .fold(0.0, f64::max);

    MIN_READABLE_COLUMN_EM
        .max(header_minimum)
        .max(body_width)
        .max(unbreakable_width)
        .max(merged)
}

/// 横向合并格需要的总宽度按跨度摊到它跨过的**每一列**上，锚点列与覆盖列同价。
/// 只记在锚点列上的话，一个跨六列的合并表头只会撑宽最左那一列，其余五列还是
/// 按 2em 的保底算，`requires_landscape` 便会把这张表当成比实际紧凑得多。
///
/// 摊分前先扣掉 `shared_padding_em`：合并格只有一份左右留白，按列平摊会让每一列
/// 都替它多算一份，跨度越大高估越狠。
fn merged_requirement(rows: &[Vec<String>], spans: &[TableSpan], column_index: usize) -> f64 {
    let mut requirement: f64 = 0.0;
    for span in spans {
        let covered = span.column..span.column + span.column_span;
        if span.column_span <= 1 || !covered.contains(&column_index) {
            continue;
        }
        let value = rows
            .get(span.row)
            .and_then(|row| row.get(span.column))
            .map_or("", String::as_str);
        if value.trim().is_empty() {
            continue;
        }
        let per_column = (merged_cell_required_width(value, span.column_span)
            - shared_padding_em(span.column_span))
        .max(0.0)
            / span.column_span as f64;
        requirement = requirement.max(per_column);
    }
    requirement
}

fn numeric_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"^-?\d+\.?\d*$",
            r"^-?\d+\.?\d*%$",
            r"^-?\d+\.?\d*[万亿千百]+$",
            r"^\d+[/-]\d+[/-]?\d*$",
            r"^\d+:\d+:?\d*$",
            r"^[\d,]+.?\d*$",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("valid numeric table pattern"))
        .collect()
    })
}

fn is_numeric(value: &str) -> bool {
    let cleaned = value.replace(|ch: char| ch.is_whitespace(), "");
    !cleaned.is_empty()
        && numeric_patterns()
            .iter()
            .any(|regex| regex.is_match(&cleaned))
}

fn is_short_digits(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.chars().count() <= NARROW_NUMERIC_MAX_DIGITS
        && value.chars().all(|ch| ch.is_ascii_digit())
}

fn has_sentence_punctuation(value: &str) -> bool {
    let width = display_width(value);
    value
        .chars()
        .any(|ch| ['。', '；', '：', '.', ';'].contains(&ch))
        || (width > SHORT_TEXT_THRESHOLD && value.chars().any(|ch| ['，', '、', ','].contains(&ch)))
}

/// 横向合并格排得下所需的宽度，**单位是 em**。
///
/// 长短的判定沿用 `display_width` 的口径（与 `analyze_table` 里的 `has_long_text`
/// 一致），但返回值一律换算成 em：下限 `MIN_READABLE_COLUMN_EM` 与上限
/// `MAX_PROSE_COLUMN_EM` 都是 em 常量，拿半角单位去跟它们比，跨两列的合并格会在
/// 20 个半角宽（合 11.1em）上封顶——只比单列 prose 的 10em 多一点，补宽等于白做。
/// 调用方按物理字宽比较时自己乘回 `CJK_WIDTH_FACTOR`。
fn merged_cell_required_width(value: &str, column_span: usize) -> f64 {
    // 花脸稿哨兵不占版面，算进宽度会让合并格凭空变宽。
    let plain = plain_cell_text(value);
    let width = display_width(&plain);
    let em = width / CJK_WIDTH_FACTOR;
    let minimum = MIN_READABLE_COLUMN_EM * column_span as f64;
    if has_sentence_punctuation(&plain) || width > LONG_TEXT_THRESHOLD {
        (em / 2.0)
            .max(minimum)
            .min(MAX_PROSE_COLUMN_EM * column_span as f64)
    } else {
        em.max(minimum)
    }
}

/// 跨 `column_span` 列的合并格白捡到的横向开销：中间那几道竖线连同两侧留白都归
/// 它用。摊分需求或估算跨列宽度时都要把这块算进去，否则放得下的合并格会被判成
/// 放不下，白白撑宽一张本来就紧的表。
fn shared_padding_em(column_span: usize) -> f64 {
    column_span.saturating_sub(1) as f64 * COLUMN_PADDING_EM
}

/// `aligns` 是 Markdown 分隔行里写明的列对齐。写了冒号的列以它为准，
/// 其余列仍按内容判定——公文表格多数不写冒号，那套启发式还是主力。
///
/// 先沿用普通表格的逐列统计，再把横向合并单元格作为“跨列总宽度约束”补进去。
/// 没有合并单元格时结果与旧算法逐位一致；合并格的需求按跨度换算到物理字宽后
/// 分摊给跨过的非定宽列，而不是只把左侧锚点列撑宽。
///
/// 注意一处刻意的不对称：普通表头再长也不进 `stats`（循环从第 2 行起），但合并
/// 表头会经 `satisfy_merged_spans` 影响列宽。合并表头是横着占版面的，不撑宽就没
/// 地方排；普通表头折行即可。这不是漏网，别顺手“修”掉。
fn analyze_table(
    rows: &[Vec<String>],
    aligns: &[ColumnAlign],
    spans: &[TableSpan],
) -> Vec<ColumnLayout> {
    let column_count = rows.first().map_or(0, Vec::len);
    if column_count == 0 {
        return Vec::new();
    }

    let mut stats = vec![
        ColumnStats {
            all_short_digits: true,
            ..ColumnStats::default()
        };
        column_count
    ];
    for (row_index, row) in rows.iter().enumerate().skip(1) {
        for (index, value) in row.iter().take(column_count).enumerate() {
            // 横向合并格的锚点和覆盖格都不进单列统计，它们的宽度另按跨度整体核算。
            let span = table_span_at(spans, row_index, index);
            if span.is_some_and(|span| !span.is_anchor(row_index, index) || span.column_span > 1) {
                continue;
            }
            if value.trim().is_empty() {
                continue;
            }
            let width = display_width(value);
            let column = &mut stats[index];
            column.total_width += width;
            column.max_width = column.max_width.max(width);
            column.count += 1;
            column.numeric_count += usize::from(is_numeric(value));
            column.has_long_text |= width > LONG_TEXT_THRESHOLD;
            column.has_punctuation |= has_sentence_punctuation(value);
            column.all_short_digits &= is_short_digits(value);
        }
    }

    let weights = stats
        .iter()
        .map(|column| {
            let average = if column.count == 0 {
                0.0
            } else {
                column.total_width / column.count as f64
            };
            column.max_width.max(average * 1.2).max(2.0)
        })
        .collect::<Vec<_>>();
    let total_weight = weights.iter().sum::<f64>();
    // 定宽数字列的"比例"记 0，不参与相对宽度分配。
    let mut ratios = stats
        .iter()
        .enumerate()
        .map(|(index, column)| {
            if column.is_fixed() {
                0.0
            } else {
                (weights[index] / total_weight * column_count as f64)
                    .clamp(MIN_WIDTH_RATIO, MAX_WIDTH_RATIO)
            }
        })
        .collect::<Vec<_>>();
    satisfy_merged_spans(rows, &stats, spans, &mut ratios);

    stats
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let numeric = column.count > 0
                && column.numeric_count as f64 / column.count as f64 >= NUMERIC_RATIO_THRESHOLD;
            let alignment = match aligns.get(index).copied().unwrap_or_default() {
                ColumnAlign::Left => ColumnAlignment::Left,
                ColumnAlign::Center => ColumnAlignment::Center,
                ColumnAlign::Right => ColumnAlignment::Right,
                ColumnAlign::Auto if numeric => ColumnAlignment::Center,
                ColumnAlign::Auto if column.has_punctuation || column.has_long_text => {
                    ColumnAlignment::Left
                }
                ColumnAlign::Auto => ColumnAlignment::Center,
            };
            let width = if column.is_fixed() {
                ColumnWidth::FixedEm(NARROW_NUMERIC_WIDTH_EM)
            } else {
                ColumnWidth::Relative((ratios[index] * 10.0 + 0.5).floor() / 10.0)
            };
            ColumnLayout { alignment, width }
        })
        .collect()
}

/// 横向合并单元格的内容必须放得进它跨过的各列之和。这里把逐列比例换算成估算
/// 物理字宽，反复把缺口分摊给跨过的非定宽列，直到所有合并格都放得下。
/// 没有合并格时循环体一次都不会进入，旧算法结果不变。
fn satisfy_merged_spans(
    rows: &[Vec<String>],
    stats: &[ColumnStats],
    spans: &[TableSpan],
    ratios: &mut [f64],
) {
    let column_count = ratios.len();
    if column_count == 0 {
        return;
    }
    let mut merges = spans
        .iter()
        .copied()
        .filter(|span| span.column_span > 1)
        .collect::<Vec<_>>();
    // 跨得多的先定：大合并格先占住它需要的宽度，小合并格再按剩余空间补缺口。
    merges.sort_by_key(|span| (std::cmp::Reverse(span.column_span), span.row, span.column));
    if merges.is_empty() {
        return;
    }

    // 版心与定宽列都不随补宽变化，整轮迭代里是常量；比例总和会变，每处理一个
    // 合并格都要重新算，否则同一个表达式里 deficit 用旧值、current 用新值。
    let available = available_content_units(stats);
    // 每加宽一次都会抬高整表比例总和，合并格实际分到的份额略小于按比例算出的值，
    // 因此多迭代几轮收敛。
    for _ in 0..8 {
        let mut changed = false;
        for span in &merges {
            let end = (span.column + span.column_span).min(column_count);
            if span.column >= end {
                continue;
            }
            let value = rows
                .get(span.row)
                .and_then(|row| row.get(span.column))
                .map_or("", String::as_str);
            if value.trim().is_empty() {
                continue;
            }
            let relative_total = relative_ratio_total(stats, ratios);
            // merged_cell_required_width 给的是 em，这里要跟物理字宽比。
            let required = merged_cell_required_width(value, end - span.column) * CJK_WIDTH_FACTOR;
            let current =
                estimated_span_width(stats, ratios, available, relative_total, span.column..end);
            if required <= current + f64::EPSILON {
                continue;
            }
            let flexible = (span.column..end)
                .filter(|index| !stats[*index].is_fixed())
                .collect::<Vec<_>>();
            let span_relative = flexible.iter().map(|index| ratios[*index]).sum::<f64>();
            // 合并格已经占满所有可分配的相对宽度时，再摊也放不下，只能作罢。
            if flexible.is_empty()
                || relative_total <= f64::EPSILON
                || relative_total - span_relative <= f64::EPSILON
            {
                continue;
            }
            // 每 1 单位比例约合 available / relative_total 个字宽，缺多少补多少比例。
            let deficit = (required - current) * relative_total / available;
            for index in flexible {
                let share = if span_relative > 0.0 {
                    ratios[index] / span_relative
                } else {
                    1.0 / (end - span.column) as f64
                };
                ratios[index] += deficit * share;
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }
}

/// 定宽数字列合起来占去的绝对版面，单位与 `display_width` 一致。
fn fixed_width_units(stats: &[ColumnStats]) -> f64 {
    stats.iter().filter(|column| column.is_fixed()).count() as f64
        * NARROW_NUMERIC_WIDTH_EM
        * CJK_WIDTH_FACTOR
}

/// 分给相对宽度列的那部分版心：扣掉单元格内边距与定宽数字列，单位同 `display_width`。
fn available_content_units(stats: &[ColumnStats]) -> f64 {
    let text_width = (PORTRAIT_TABLE_WIDTH_EM - stats.len() as f64 * COLUMN_PADDING_EM).max(0.0)
        * CJK_WIDTH_FACTOR;
    (text_width - fixed_width_units(stats)).max(1.0)
}

/// 参与相对宽度分配的各列比例之和；定宽数字列不在其中。
fn relative_ratio_total(stats: &[ColumnStats], ratios: &[f64]) -> f64 {
    ratios
        .iter()
        .enumerate()
        .filter(|(index, _)| !stats[*index].is_fixed())
        .map(|(_, ratio)| *ratio)
        .sum()
}

/// 估算一个合并格跨过这几列后能用多宽：定宽列按绝对宽度，其余按比例分摊可用
/// 版面，再加上被它吞掉的那几份列内边距（见 `shared_padding_em`）。
fn estimated_span_width(
    stats: &[ColumnStats],
    ratios: &[f64],
    available: f64,
    relative_total: f64,
    columns: Range<usize>,
) -> f64 {
    let column_span = columns.len();
    let content = columns
        .map(|index| {
            if stats[index].is_fixed() {
                NARROW_NUMERIC_WIDTH_EM * CJK_WIDTH_FACTOR
            } else if relative_total > 0.0 {
                available * ratios[index] / relative_total
            } else {
                0.0
            }
        })
        .sum::<f64>();
    content + shared_padding_em(column_span) * CJK_WIDTH_FACTOR
}

pub(super) fn to_docx_grid(
    rows: &[Vec<String>],
    aligns: &[ColumnAlign],
    spans: &[TableSpan],
    total_width_twips: usize,
    em_width_twips: usize,
) -> (Vec<usize>, Vec<ColumnAlignment>) {
    let columns = analyze_table(rows, aligns, spans);
    let fixed = columns
        .iter()
        .map(|column| match column.width {
            ColumnWidth::FixedEm(em) => (em * em_width_twips as f64).round() as usize,
            ColumnWidth::Relative(_) => 0,
        })
        .collect::<Vec<_>>();
    let fixed_total = fixed.iter().sum::<usize>();
    let relative_total = columns
        .iter()
        .filter_map(|column| match column.width {
            ColumnWidth::Relative(ratio) => Some(ratio),
            ColumnWidth::FixedEm(_) => None,
        })
        .sum::<f64>();
    let last_relative = columns
        .iter()
        .rposition(|column| matches!(column.width, ColumnWidth::Relative(_)));
    let remaining = total_width_twips.saturating_sub(fixed_total);
    let mut allocated = 0usize;
    let grid = columns
        .iter()
        .enumerate()
        .map(|(index, column)| match column.width {
            ColumnWidth::FixedEm(_) => fixed[index],
            ColumnWidth::Relative(_) if Some(index) == last_relative => remaining - allocated,
            ColumnWidth::Relative(ratio) => {
                let width = (remaining as f64 * ratio / relative_total).round() as usize;
                allocated += width;
                width
            }
        })
        .collect();
    let alignments = columns.iter().map(|column| column.alignment).collect();
    (grid, alignments)
}

/// 返回单元格最终的水平对齐。普通/纵向合并单元格沿用所在列；横向合并跨过的
/// 各列对齐一致时继承该值，否则按合并单元格自身内容判定。
///
/// `numbered` 是序号表：它的分组行（整行跨度）按公文习惯靠左，其余表格的
/// 整行合并格不受影响，仍按下面的规则判定。
pub(crate) fn resolve_cell_alignment(
    rows: &[Vec<String>],
    spans: &[TableSpan],
    column_alignments: &[ColumnAlignment],
    numbered: bool,
    row: usize,
    column: usize,
) -> ColumnAlignment {
    if row == 0 {
        return ColumnAlignment::Center;
    }
    let fallback = column_alignments
        .get(column)
        .copied()
        .unwrap_or(ColumnAlignment::Left);
    let Some(span) = table_span_at(spans, row, column) else {
        return fallback;
    };
    if span.column_span <= 1 {
        return fallback;
    }
    if numbered && span.column == 0 && span.column_span >= column_alignments.len() {
        return ColumnAlignment::Left;
    }
    let end = (span.column + span.column_span).min(column_alignments.len());
    let covered = &column_alignments[span.column.min(end)..end];
    if covered.is_empty() {
        return fallback;
    }
    if covered.iter().all(|alignment| *alignment == covered[0]) {
        return covered[0];
    }
    // 花脸稿哨兵不占版面，判断内容性质前先滤掉。
    let plain = plain_cell_text(
        rows.get(span.row)
            .and_then(|row| row.get(span.column))
            .map_or("", String::as_str),
    );
    if is_numeric(&plain)
        || (!has_sentence_punctuation(&plain) && display_width(&plain) <= LONG_TEXT_THRESHOLD)
    {
        ColumnAlignment::Center
    } else {
        ColumnAlignment::Left
    }
}

pub(super) fn to_longtblr(
    rows: &[Vec<String>],
    aligns: &[ColumnAlign],
    spans: &[TableSpan],
    numbered: bool,
) -> String {
    let columns = analyze_table(rows, aligns, spans);
    if rows.is_empty() || columns.is_empty() {
        return String::new();
    }
    // 规格 §6：表头含“姓名/联系人”的列，非表头单元格按版记的方式处理姓名宽度。
    let name_column = rows.first().and_then(|header| {
        header
            .iter()
            .position(|cell| cell.contains("姓名") || cell.contains("联系人"))
    });
    let colspec = columns
        .iter()
        .map(|column| {
            let align = match column.alignment {
                ColumnAlignment::Left => 'l',
                ColumnAlignment::Center => 'c',
                ColumnAlignment::Right => 'r',
            };
            // 竖向一律居中（`m`），跟 DOCX 的 vertical_align 与预览的画法对齐。
            // tabularray 的默认值不写在规格里，别赖它——纵向合并一出来就看得见。
            match column.width {
                ColumnWidth::FixedEm(em) => format!("Q[{align},m,wd={em:.0}em]"),
                ColumnWidth::Relative(1.0) => format!("X[{align},m]"),
                ColumnWidth::Relative(ratio) => format!("X[{ratio:.1},{align},m]"),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let column_alignments = columns
        .iter()
        .map(|column| column.alignment)
        .collect::<Vec<_>>();

    let mut output = format!(
        "\\begin{{longtblr}}[\n  label = none,\n  entry = none,\n]{{\n  colspec = {{{colspec}}},\n  rowhead = 1,\n  hlines,\n  vlines,\n  row{{1}} = {{c, font=\\heiti\\enheiti}},\n}}\n"
    );
    for (row_index, row) in rows.iter().enumerate() {
        let cells = row
            .iter()
            .enumerate()
            .map(|(column_index, cell)| {
                let span = table_span_at(spans, row_index, column_index);
                if span.is_some_and(|span| !span.is_anchor(row_index, column_index)) {
                    return String::new();
                }
                let segments = inline_segments(cell);
                // 姓名列要按字数算宽度，哨兵会让 2 字姓名被当成 4 字。这里先滤掉。
                let cleaned = segments
                    .iter()
                    .map(|segment| segment.text.as_str())
                    .collect::<String>()
                    .chars()
                    .filter(|ch| !crate::export::is_redline_sentinel(*ch))
                    .collect::<String>();
                let escaped = if name_column == Some(column_index) && row_index > 0 {
                    let name = latex_name(&cleaned);
                    if segments.iter().any(|segment| segment.bold) {
                        format!("\\GwBold{{{name}}}")
                    } else {
                        name
                    }
                } else if row_index == 0 {
                    tex_escape(&cleaned)
                } else {
                    // 单元格也要能画花脸稿标记：表格里改了一个数字、一个时限，
                    // 恰恰是最需要让领导一眼看见的地方。先按哨兵切块，块内再走
                    // 原来的加粗逻辑；没有哨兵时就是一整块，与从前完全一致。
                    redline_chunks(cell)
                        .into_iter()
                        .map(|chunk| {
                            let inner = inline_segments(&chunk.text)
                                .iter()
                                .map(|segment| {
                                    let escaped = tex_escape(&segment.text);
                                    if segment.bold {
                                        format!("\\GwBold{{{escaped}}}")
                                    } else {
                                        escaped
                                    }
                                })
                                .collect::<String>();
                            match chunk.kind {
                                RedlineKind::Same => inner,
                                RedlineKind::Deleted => format!("\\GwDel{{{inner}}}"),
                                RedlineKind::Added => format!("\\GwAdd{{{inner}}}"),
                            }
                        })
                        .collect::<String>()
                };
                let content = if row_index == 0 {
                    format!("\\heiti\\enheiti {escaped}")
                } else {
                    escaped
                };
                if let Some(span) = span {
                    let align = match resolve_cell_alignment(
                        rows,
                        spans,
                        &column_alignments,
                        numbered,
                        row_index,
                        column_index,
                    ) {
                        ColumnAlignment::Left => 'l',
                        ColumnAlignment::Center => 'c',
                        ColumnAlignment::Right => 'r',
                    };
                    let mut options = Vec::new();
                    if span.row_span > 1 {
                        options.push(format!("r={}", span.row_span));
                    }
                    if span.column_span > 1 {
                        options.push(format!("c={}", span.column_span));
                    }
                    // 只写水平对齐；竖向居中由 colspec 里的 `m` 统一管，
                    // 与 `\SetCell[c=2]{c}` 的官方写法一致。
                    format!("\\SetCell[{}]{{{align}}} {content}", options.join(","))
                } else {
                    content
                }
            })
            .collect::<Vec<_>>()
            .join(" & ");
        output.push_str(&cells);
        output.push_str(" \\\\\n");
    }
    output.push_str("\\end{longtblr}");
    output
}

/// 规格 §3.2/§6 姓名宽度：2 字姓名中间加 1em，4 字姓名压缩到 3 字宽，保证视觉对齐。
fn latex_name(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    match chars.len() {
        2 => format!("{}\\hspace{{1em}}{}", chars[0], chars[1]),
        4 => format!("\\resizebox{{3em}}{{0.9em}}{{{}}}", tex_escape(value)),
        _ => tex_escape(value),
    }
}

fn tex_escape(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => output.push_str("\\textbackslash{}"),
            '{' => output.push_str("\\{"),
            '}' => output.push_str("\\}"),
            '#' => output.push_str("\\#"),
            '$' => output.push_str("\\$"),
            '%' => output.push_str("\\%"),
            '&' => output.push_str("\\&"),
            '_' => output.push_str("\\_"),
            '^' => output.push_str("\\textasciicircum{}"),
            '~' => output.push_str("\\textasciitilde{}"),
            _ => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Vec<String>> {
        vec![
            vec!["序号".into(), "名称".into(), "详细说明".into()],
            vec!["1".into(), "短项".into(), "这是一段很长的说明文字。".into()],
            vec!["2".into(), "另一项".into(), "另一段较长的说明文字。".into()],
        ]
    }

    #[test]
    fn narrow_numeric_column_is_fixed_and_docx_fills_width() {
        let (grid, _) = to_docx_grid(&rows(), &[], &[], 8_844, 280);
        assert_eq!(grid[0], 560);
        assert_eq!(grid.iter().sum::<usize>(), 8_844);
        assert!(grid[2] > grid[1]);
    }

    /// 分隔行里写了冒号的列以冒号为准，其余列仍按内容判定。
    #[test]
    fn explicit_alignment_overrides_the_heuristic() {
        // 不写冒号时首列是数字列，智能列宽判它居中。
        let (_, alignments) = to_docx_grid(&rows(), &[], &[], 8_844, 280);
        assert_eq!(alignments[0], ColumnAlignment::Center);
        assert_eq!(alignments[2], ColumnAlignment::Left, "长说明列左对齐");

        let aligns = [ColumnAlign::Left, ColumnAlign::Auto, ColumnAlign::Right];
        let (_, alignments) = to_docx_grid(&rows(), &aligns, &[], 8_844, 280);
        assert_eq!(alignments[0], ColumnAlignment::Left);
        assert_eq!(alignments[2], ColumnAlignment::Right);
        // 没写冒号的第二列不受影响，仍按内容判定。
        assert_eq!(alignments[1], ColumnAlignment::Center);

        // TeX 的 colspec 跟着换成 l / r。
        let tex = to_longtblr(&rows(), &aligns, &[], false);
        let colspec = tex
            .lines()
            .find(|line| line.contains("colspec"))
            .expect("有 colspec");
        assert!(colspec.contains("[l,"), "首列应左对齐：{colspec}");
        assert!(colspec.contains(",r,m]"), "末列应右对齐：{colspec}");
    }

    #[test]
    fn tex_uses_longtblr_with_matching_smart_columns() {
        let tex = to_longtblr(&rows(), &[], &[], false);
        assert!(tex.contains("\\begin{longtblr}"));
        assert!(tex.contains("label = none"));
        assert!(tex.contains("entry = none"));
        assert!(!tex.contains("caption ="));
        assert!(tex.contains("Q[c,m,wd=2em]"));
        assert!(tex.contains("X["));
        assert!(tex.contains("rowhead = 1"));
    }

    #[test]
    fn tex_emits_horizontal_and_vertical_spans() {
        let table = vec![
            vec!["类别".into(), "项目".into(), "说明".into()],
            vec!["横向".into(), String::new(), "备注".into()],
            vec!["纵向".into(), "事项一".into(), "甲".into()],
            vec![String::new(), "事项二".into(), "乙".into()],
        ];
        let spans = [
            TableSpan {
                row: 1,
                column: 0,
                row_span: 1,
                column_span: 2,
            },
            TableSpan {
                row: 2,
                column: 0,
                row_span: 2,
                column_span: 1,
            },
        ];
        let tex = to_longtblr(&table, &[], &spans, false);
        assert!(tex.contains("\\SetCell[c=2]{c} 横向"), "{tex}");
        assert!(tex.contains("\\SetCell[r=2]{c} 纵向"), "{tex}");
        assert!(!tex.contains("横向 & 备注"), "被覆盖格不得重复内容：{tex}");
    }

    #[test]
    fn landscape_decision_combines_column_count_and_content_width() {
        let compact = vec![
            vec![
                "序号".into(),
                "甲".into(),
                "乙".into(),
                "丙".into(),
                "丁".into(),
                "戊".into(),
                "己".into(),
                "庚".into(),
            ],
            vec![
                "1".into(),
                "是".into(),
                "否".into(),
                "是".into(),
                "否".into(),
                "是".into(),
                "否".into(),
                "是".into(),
            ],
        ];
        assert!(!requires_landscape(&compact, &[]));

        let crowded = vec![
            vec![
                "序号".into(),
                "事项类别".into(),
                "事项名称".into(),
                "存在问题".into(),
                "整改措施".into(),
                "责任部门".into(),
                "完成时限".into(),
                "当前状态".into(),
            ],
            vec![
                "1".into(),
                "线上办理".into(),
                "行政备案事项".into(),
                "移动端部分页面显示不完整，申请人无法正常上传附件。".into(),
                "优化移动端页面适配，增加格式和大小提示并开展测试。".into(),
                "技术保障部门".into(),
                "2026年8月12日".into(),
                "已完成".into(),
            ],
        ];
        assert!(requires_landscape(&crowded, &[]));
    }

    #[test]
    fn many_rows_alone_do_not_trigger_landscape() {
        let mut table = vec![vec!["序号".into(), "名称".into()]];
        for index in 1..=100 {
            table.push(vec![index.to_string(), "短项".into()]);
        }
        assert!(!requires_landscape(&table, &[]));
    }

    #[test]
    fn tex_table_preserves_markdown_bold_and_normalizes_quotes() {
        let table = vec![
            vec!["项目".into(), "说明".into()],
            vec!["甲".into(), "\"**重点**\"内容".into()],
        ];
        let tex = to_longtblr(&table, &[], &[], false);
        assert!(tex.contains("“\\GwBold{重点}”内容"), "{tex}");
        assert!(!tex.contains("**"));
    }

    #[test]
    fn name_column_formats_two_and_four_character_names() {
        let table = vec![
            vec!["序号".into(), "姓名".into()],
            vec!["1".into(), "张三".into()],
            vec!["2".into(), "王小明".into()],
            vec!["3".into(), "欧阳翠花".into()],
        ];
        let tex = to_longtblr(&table, &[], &[], false);
        // 2 字姓名中间加 1em。
        assert!(tex.contains("张\\hspace{1em}三"));
        // 4 字姓名压缩到 3 字宽。
        assert!(tex.contains("\\resizebox{3em}{0.9em}{欧阳翠花}"));
        // 3 字姓名原样。
        assert!(tex.contains("王小明"));
    }

    /// 合并格跨过的列本来就放得下内容时，不得把锚点列撑宽、把别的列挤窄。
    /// 旧算法直接拿合并格的字符数与列权重相加，会把首列撑到末列的两倍。
    #[test]
    fn merged_cell_does_not_widen_columns_that_already_fit() {
        let table = vec![
            vec!["甲".into(), "乙".into(), "丙".into(), "丁".into()],
            vec!["一".into(), "二".into(), "三".into(), "四".into()],
            vec![
                "横跨两列的一段较长说明文字。".into(),
                String::new(),
                "末".into(),
                "尾".into(),
            ],
        ];
        let spans = [TableSpan {
            row: 2,
            column: 0,
            row_span: 1,
            column_span: 2,
        }];
        let (grid, _) = to_docx_grid(&table, &[], &spans, 8_844, 280);
        assert!(
            grid[0].abs_diff(grid[3]) < 400,
            "合并格不该把锚点列撑宽：{grid:?}"
        );
    }

    /// 合并格确实放不下时才补宽，而且只补到刚好放得下，不是无上限地撑大。
    #[test]
    fn merged_span_widens_its_columns_only_as_much_as_needed() {
        let table = vec![
            vec![
                "甲".into(),
                "乙".into(),
                "丙".into(),
                "丁".into(),
                "戊".into(),
                "己".into(),
            ],
            vec![
                "一".into(),
                "二".into(),
                "三".into(),
                "四".into(),
                "五".into(),
                "六".into(),
            ],
            vec![
                "这是一段需要跨两列才能排得下的合并说明文字。".into(),
                String::new(),
                "末".into(),
                "尾".into(),
                "甲".into(),
                "乙".into(),
            ],
        ];
        let spans = [TableSpan {
            row: 2,
            column: 0,
            row_span: 1,
            column_span: 2,
        }];
        let (grid, _) = to_docx_grid(&table, &[], &spans, 8_844, 280);
        let merged_width = grid[0] + grid[1];
        // 合并格要放得下约 20 个字宽（约 3100 twips）。
        assert!(merged_width >= 3_000, "合并格没被补宽：{grid:?}");
        // 但也不能为了它把其余列挤到没法看。
        assert!(merged_width < 4_000, "合并格被补得过宽：{grid:?}");
    }

    /// 定宽数字列不参与合并格的补宽，缺口只落到跨过的非定宽列上。
    #[test]
    fn merged_span_pushes_width_into_flexible_columns_not_fixed_numeric_ones() {
        let prose = "这是一段较长的说明文字用于占满后两列的宽度。";
        let table = vec![
            vec!["序号".into(), "项目".into(), "说明".into(), "备注".into()],
            vec!["1".into(), "甲".into(), prose.into(), prose.into()],
            vec!["2".into(), "乙".into(), prose.into(), prose.into()],
            vec![
                "跨列合并的标题需要写得很长很长以便把两列的宽度都撑开一些。".into(),
                String::new(),
                "末".into(),
                "尾".into(),
            ],
        ];
        let spans = [TableSpan {
            row: 3,
            column: 0,
            row_span: 1,
            column_span: 2,
        }];
        let (grid, _) = to_docx_grid(&table, &[], &spans, 8_844, 280);
        assert_eq!(grid[0], 560, "数字列必须保持 2em 定宽：{grid:?}");
        assert!(grid[1] > grid[2], "缺口应落到跨过的非定宽列：{grid:?}");
    }

    /// 合并格的需求以 em 计，上限随跨度放大。拿 `display_width` 的半角宽去跟
    /// `MAX_PROSE_COLUMN_EM` 比的话，跨两列会在 20 个半角宽（合 11.1em）上封顶——
    /// 只比单列 prose 的 10em 多一点，再长的字也换不来更宽的格子。
    #[test]
    fn merged_requirement_is_in_em_and_scales_with_span() {
        let short = merged_cell_required_width(&"说".repeat(20), 2);
        let long = merged_cell_required_width(&"说".repeat(40), 2);
        assert!(long > short, "字更多就要更宽：{short} → {long}");
        assert!(
            (long - MAX_PROSE_COLUMN_EM * 2.0).abs() < 1e-6,
            "跨两列的上限是 20em：{long}"
        );
        let single = merged_cell_required_width(&"说".repeat(40), 1);
        assert!(
            (single - MAX_PROSE_COLUMN_EM).abs() < 1e-6,
            "同样的字放进单列，上限仍是 10em：{single}"
        );
    }

    /// 上一条的版面后果：同一张表里把合并格的字加长，它跨过的两列必须真的变宽。
    #[test]
    fn a_longer_merged_cell_really_gets_a_wider_span() {
        let span_width = |chars: usize| {
            let table = vec![
                vec![
                    "甲".into(),
                    "乙".into(),
                    "丙".into(),
                    "丁".into(),
                    "戊".into(),
                    "己".into(),
                ],
                vec![
                    "一".into(),
                    "二".into(),
                    "三".into(),
                    "四".into(),
                    "五".into(),
                    "六".into(),
                ],
                vec![
                    "说".repeat(chars),
                    String::new(),
                    "末".into(),
                    "尾".into(),
                    "甲".into(),
                    "乙".into(),
                ],
            ];
            let spans = [TableSpan {
                row: 2,
                column: 0,
                row_span: 1,
                column_span: 2,
            }];
            let (grid, _) = to_docx_grid(&table, &[], &spans, 8_844, 280);
            grid[0] + grid[1]
        };
        assert!(
            span_width(40) > span_width(22) + 500,
            "{} → {}",
            span_width(22),
            span_width(40)
        );
    }

    /// 合并表头的需求摊到跨过的每一列上，真放不下时照样判横排。
    /// 只记在锚点列上的话，这张表会被当成比实际紧凑得多，漏掉横排。
    #[test]
    fn a_merged_header_that_cannot_fit_still_forces_landscape() {
        let mut header = vec!["序号".to_string(), "关于".repeat(30)];
        header.resize(8, String::new());
        let table = vec![
            header,
            vec![
                "1".into(),
                "甲".into(),
                "乙".into(),
                "丙".into(),
                "丁".into(),
                "戊".into(),
                "己".into(),
                "庚".into(),
            ],
        ];
        let spans = [TableSpan {
            row: 0,
            column: 1,
            row_span: 1,
            column_span: 7,
        }];
        assert!(requires_landscape(&table, &spans));
    }

    /// 合并表头只算一次：把需求按跨度摊到跨过的列上，紧凑表不会被误判成需要横排。
    #[test]
    fn merged_header_does_not_force_a_compact_table_into_landscape() {
        let table = vec![
            vec![
                "序号".into(),
                "关于重点项目推进情况与存在问题及下一步整改措施的统计表".into(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                "备注".into(),
            ],
            vec![
                "1".into(),
                "甲".into(),
                "乙".into(),
                "丙".into(),
                "丁".into(),
                "戊".into(),
                "己".into(),
                "庚".into(),
            ],
        ];
        let spans = [TableSpan {
            row: 0,
            column: 1,
            row_span: 1,
            column_span: 6,
        }];
        assert!(!requires_landscape(&table, &spans), "合并表头不该触发横排");
        // 同一张表若按单列硬算，表头会把一列撑到 13em，误判成需要横排。
        let mut narrow = table.clone();
        for cell in &mut narrow[0][2..7] {
            *cell = String::new();
        }
        assert!(requires_landscape(&narrow, &[]));
    }

    /// 横向合并格跨过的各列对齐一致时继承该值，不一致时按内容判定。
    #[test]
    fn merged_cell_alignment_follows_covered_columns_or_content() {
        let table = vec![
            vec!["甲".into(), "乙".into(), "丙".into()],
            vec!["合并".into(), String::new(), "丁".into()],
        ];
        let spans = [TableSpan {
            row: 1,
            column: 0,
            row_span: 1,
            column_span: 2,
        }];
        let alignments = [
            ColumnAlignment::Center,
            ColumnAlignment::Center,
            ColumnAlignment::Left,
        ];
        assert_eq!(
            resolve_cell_alignment(&table, &spans, &alignments, false, 1, 0),
            ColumnAlignment::Center
        );

        let alignments = [
            ColumnAlignment::Left,
            ColumnAlignment::Right,
            ColumnAlignment::Left,
        ];
        // 跨过的列对齐不一致，横向合并格按自身内容判定：短文本居中。
        assert_eq!(
            resolve_cell_alignment(&table, &spans, &alignments, false, 1, 0),
            ColumnAlignment::Center
        );
    }

    /// 序号表的分组行（整行合并）靠左；同样的整行合并格出现在普通表格里
    /// 仍按内容判定——「合计」这类行不会被顺手改成左对齐。
    #[test]
    fn numbered_group_row_is_left_aligned_but_plain_tables_are_not() {
        let table = vec![
            vec!["序号".into(), "标题".into(), "内容".into()],
            vec!["（一）大标题".into(), String::new(), String::new()],
            vec!["合计".into(), String::new(), String::new()],
        ];
        let spans = [
            TableSpan {
                row: 1,
                column: 0,
                row_span: 1,
                column_span: 3,
            },
            TableSpan {
                row: 2,
                column: 0,
                row_span: 1,
                column_span: 3,
            },
        ];
        let alignments = [
            ColumnAlignment::Center,
            ColumnAlignment::Left,
            ColumnAlignment::Left,
        ];
        assert_eq!(
            resolve_cell_alignment(&table, &spans, &alignments, true, 1, 0),
            ColumnAlignment::Left
        );
        assert_eq!(
            resolve_cell_alignment(&table, &spans, &alignments, false, 1, 0),
            ColumnAlignment::Center,
            "普通表格的整行合并格仍按内容判定"
        );
        // 非整行跨度的横向合并格不受序号表影响。
        let narrow = [TableSpan {
            row: 1,
            column: 0,
            row_span: 1,
            column_span: 2,
        }];
        assert_eq!(
            resolve_cell_alignment(&table, &narrow, &alignments, true, 1, 0),
            ColumnAlignment::Center
        );
    }

    /// TeX 那边同样按序号表的规矩给分组行写左对齐。
    #[test]
    fn tex_aligns_numbered_group_rows_to_the_left() {
        let table = vec![
            vec!["序号".into(), "标题".into(), "内容".into()],
            vec!["（一）大标题".into(), String::new(), String::new()],
            vec!["标题1".into(), "内容1".into(), String::new()],
        ];
        let spans = [TableSpan {
            row: 1,
            column: 0,
            row_span: 1,
            column_span: 3,
        }];
        let tex = to_longtblr(&table, &[], &spans, true);
        assert!(tex.contains("\\SetCell[c=3]{l} （一）大标题"), "{tex}");
        let plain = to_longtblr(&table, &[], &spans, false);
        assert!(plain.contains("\\SetCell[c=3]{c} （一）大标题"), "{plain}");
    }
}
