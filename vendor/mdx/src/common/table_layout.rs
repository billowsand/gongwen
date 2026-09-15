//! Shared table column analysis and width allocation.
//!
//! This is the canonical implementation originally used by the research TeX
//! emitter. All TeX and DOCX emitters consume the same analysis result.

/// Analysis thresholds inherited from the research TeX table algorithm.
const SHORT_TEXT_THRESHOLD: f64 = 8.0;
const LONG_TEXT_THRESHOLD: f64 = 20.0;
const NUMERIC_RATIO_THRESHOLD: f64 = 0.8;
const MIN_WIDTH_RATIO: f64 = 0.8;
const MAX_WIDTH_RATIO: f64 = 4.0;
const CJK_WIDTH_FACTOR: f64 = 1.8;
const NARROW_NUMERIC_MAX_DIGITS: usize = 2;

/// A narrow sequence-number column is fixed at this many ems.
pub const NARROW_NUMERIC_WIDTH_EM: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnAlignment {
    Left,
    Center,
}

impl ColumnAlignment {
    pub fn latex(self) -> char {
        match self {
            Self::Left => 'l',
            Self::Center => 'c',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnWidth {
    /// Flexible column width, expressed as a relative X-column weight.
    Relative(f64),
    /// Fixed column width, expressed in ems.
    FixedEm(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnLayout {
    pub alignment: ColumnAlignment,
    pub width: ColumnWidth,
    pub avg_display_width: f64,
    pub max_display_width: f64,
    pub is_numeric: bool,
    pub has_punctuation: bool,
}

#[derive(Debug, Clone)]
struct ColumnStats {
    total_width: f64,
    max_width: f64,
    count: usize,
    numeric_count: usize,
    has_long_text: bool,
    has_punctuation: bool,
    avg_width: f64,
    is_numeric: bool,
    is_narrow_numeric: bool,
}

impl ColumnStats {
    fn new() -> Self {
        Self {
            total_width: 0.0,
            max_width: 0.0,
            count: 0,
            numeric_count: 0,
            has_long_text: false,
            has_punctuation: false,
            avg_width: 0.0,
            is_numeric: false,
            is_narrow_numeric: false,
        }
    }
}

fn calc_display_width(s: &str) -> f64 {
    s.chars()
        .map(|c| if c.is_ascii() { 1.0 } else { CJK_WIDTH_FACTOR })
        .sum()
}

fn is_numeric_content(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    let cleaned = s.replace(|c: char| c.is_whitespace(), "").replace('\\', "");
    let patterns = [
        r"^-?\d+\.?\d*$",
        r"^-?\d+\.?\d*%$",
        r"^-?\d+\.?\d*[万亿千百]+$",
        r"^\d+[/-]\d+[/-]?\d*$",
        r"^\d+:\d+:?\d*$",
        r"^[\d,]+.?\d*$",
    ];

    patterns.iter().any(|pattern| {
        regex::Regex::new(pattern)
            .map(|re| re.is_match(&cleaned))
            .unwrap_or(false)
    })
}

fn has_sentence_punctuation(s: &str) -> bool {
    let short_text_punctuations = ['，', '、', ','];
    let long_text_punctuations = ['。', '；', '：', '.', ';'];
    let width = calc_display_width(s);
    let has_long_punct = s.chars().any(|c| long_text_punctuations.contains(&c));
    let has_short_punct = s.chars().any(|c| short_text_punctuations.contains(&c));

    has_long_punct || (has_short_punct && width > SHORT_TEXT_THRESHOLD)
}

fn is_short_digits(s: &str) -> bool {
    let text = s.trim();
    !text.is_empty()
        && text.chars().count() <= NARROW_NUMERIC_MAX_DIGITS
        && text.chars().all(|c| c.is_ascii_digit())
}

fn analyze_column(cells: &[String]) -> ColumnStats {
    let mut stats = ColumnStats::new();
    let mut all_short_digits = true;

    for cell_text in cells {
        if cell_text.is_empty() {
            continue;
        }

        if !is_short_digits(cell_text) {
            all_short_digits = false;
        }

        let width = calc_display_width(cell_text);
        stats.total_width += width;
        stats.count += 1;
        stats.max_width = stats.max_width.max(width);
        stats.numeric_count += usize::from(is_numeric_content(cell_text));
        stats.has_long_text |= width > LONG_TEXT_THRESHOLD;
        stats.has_punctuation |= has_sentence_punctuation(cell_text);
    }

    stats.avg_width = if stats.count > 0 {
        stats.total_width / stats.count as f64
    } else {
        0.0
    };
    stats.is_numeric = stats.count > 0
        && (stats.numeric_count as f64 / stats.count as f64) >= NUMERIC_RATIO_THRESHOLD;
    stats.is_narrow_numeric = stats.count > 0 && all_short_digits;
    stats
}

fn determine_alignment(stats: &ColumnStats) -> ColumnAlignment {
    if stats.is_numeric {
        return ColumnAlignment::Center;
    }
    if stats.has_punctuation || stats.has_long_text {
        return ColumnAlignment::Left;
    }
    if stats.avg_width <= LONG_TEXT_THRESHOLD {
        return ColumnAlignment::Center;
    }
    ColumnAlignment::Left
}

fn calculate_width_ratios(columns_stats: &[ColumnStats]) -> Vec<f64> {
    let num_cols = columns_stats.len();
    if num_cols == 0 {
        return Vec::new();
    }

    let weights: Vec<f64> = columns_stats
        .iter()
        .map(|stats| stats.max_width.max(stats.avg_width * 1.2).max(2.0))
        .collect();
    let total_weight: f64 = weights.iter().sum();

    weights
        .iter()
        .map(|weight| {
            let ratio =
                (weight / total_weight * num_cols as f64).clamp(MIN_WIDTH_RATIO, MAX_WIDTH_RATIO);
            (ratio * 10.0 + 0.5).floor() / 10.0
        })
        .collect()
}

/// Analyze table body cells (the first row is the header) using the canonical
/// research TeX algorithm.
pub fn analyze_table(rows: &[Vec<String>]) -> Vec<ColumnLayout> {
    let num_cols = rows.first().map_or(0, Vec::len);
    if num_cols == 0 {
        return Vec::new();
    }

    let mut columns = vec![Vec::new(); num_cols];
    for row in rows.iter().skip(1) {
        for (col_idx, cell) in row.iter().take(num_cols).enumerate() {
            columns[col_idx].push(cell.clone());
        }
    }

    let stats: Vec<ColumnStats> = columns.iter().map(|cells| analyze_column(cells)).collect();
    let ratios = calculate_width_ratios(&stats);

    stats
        .iter()
        .enumerate()
        .map(|(index, stats)| ColumnLayout {
            alignment: determine_alignment(stats),
            width: if stats.is_narrow_numeric {
                ColumnWidth::FixedEm(NARROW_NUMERIC_WIDTH_EM)
            } else {
                ColumnWidth::Relative(ratios[index])
            },
            avg_display_width: stats.avg_width,
            max_display_width: stats.max_width,
            is_numeric: stats.is_numeric,
            has_punctuation: stats.has_punctuation,
        })
        .collect()
}

/// Convert the shared column layout to fixed widths for DOCX.
///
/// Fixed-em columns are reserved first; relative columns divide the remaining
/// content width using exactly the weights emitted as TeX X columns.
pub fn to_docx_grid(
    columns: &[ColumnLayout],
    total_width_twips: usize,
    em_width_twips: usize,
) -> Vec<usize> {
    if columns.is_empty() {
        return Vec::new();
    }

    let fixed_widths: Vec<usize> = columns
        .iter()
        .map(|column| match column.width {
            ColumnWidth::FixedEm(em) => (em * em_width_twips as f64).round() as usize,
            ColumnWidth::Relative(_) => 0,
        })
        .collect();
    let fixed_total: usize = fixed_widths.iter().sum();
    let relative_total: f64 = columns
        .iter()
        .filter_map(|column| match column.width {
            ColumnWidth::Relative(ratio) => Some(ratio),
            ColumnWidth::FixedEm(_) => None,
        })
        .sum();

    if relative_total == 0.0 {
        return fixed_widths;
    }

    let remaining = total_width_twips.saturating_sub(fixed_total);
    let last_relative = columns
        .iter()
        .rposition(|column| matches!(column.width, ColumnWidth::Relative(_)))
        .expect("relative_total is non-zero");
    let mut allocated = 0usize;

    columns
        .iter()
        .enumerate()
        .map(|(index, column)| match column.width {
            ColumnWidth::FixedEm(_) => fixed_widths[index],
            ColumnWidth::Relative(_) if index == last_relative => remaining - allocated,
            ColumnWidth::Relative(ratio) => {
                let width = (remaining as f64 * ratio / relative_total).round() as usize;
                allocated += width;
                width
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_counts_cjk_as_research_tex_did() {
        assert_eq!(calc_display_width("hello"), 5.0);
        assert_eq!(calc_display_width("你好"), 3.6);
        assert_eq!(calc_display_width("hello你好"), 8.6);
    }

    #[test]
    fn narrow_numeric_column_is_fixed_at_two_em() {
        let rows = vec![
            vec!["序号".into(), "说明".into()],
            vec!["1".into(), "第一条说明文字，较长。".into()],
            vec!["12".into(), "第二条说明文字，同样较长。".into()],
        ];
        let layout = analyze_table(&rows);
        assert_eq!(layout[0].width, ColumnWidth::FixedEm(2.0));
        assert!(matches!(layout[1].width, ColumnWidth::Relative(_)));
    }

    #[test]
    fn docx_grid_reserves_fixed_columns_and_fills_available_width() {
        let rows = vec![
            vec!["序号".into(), "名称".into(), "详细说明".into()],
            vec![
                "1".into(),
                "短项".into(),
                "这是一段很长的说明文字，用于拉宽最后一列。".into(),
            ],
            vec!["2".into(), "另一项".into(), "另一段较长的说明文字。".into()],
        ];
        let layout = analyze_table(&rows);
        let grid = to_docx_grid(&layout, 8_844, 280);

        assert_eq!(grid[0], 560);
        assert_eq!(grid.iter().sum::<usize>(), 8_844);
        assert!(grid[2] > grid[1]);
    }
}
