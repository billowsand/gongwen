//! 与 mdx official/research 共用思路的智能表格列宽分析。

use super::inline_segments;
use regex::Regex;
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

fn display_width(value: &str) -> f64 {
    value
        .chars()
        .map(|ch| if ch.is_ascii() { 1.0 } else { CJK_WIDTH_FACTOR })
        .sum()
}

fn plain_cell_text(value: &str) -> String {
    inline_segments(value)
        .iter()
        .map(|segment| segment.text.as_str())
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
pub(super) fn requires_landscape(rows: &[Vec<String>]) -> bool {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if column_count <= 1 {
        return false;
    }

    let mut required_content_width = 0.0;
    for column_index in 0..column_count {
        let header = rows
            .first()
            .and_then(|row| row.get(column_index))
            .map_or("", String::as_str);
        let body = rows
            .iter()
            .skip(1)
            .filter_map(|row| row.get(column_index))
            .filter(|cell| !cell.trim().is_empty())
            .collect::<Vec<_>>();

        if !body.is_empty()
            && body
                .iter()
                .all(|cell| is_short_digits(&plain_cell_text(cell)))
        {
            required_content_width += NARROW_NUMERIC_WIDTH_EM;
            continue;
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

        required_content_width += MIN_READABLE_COLUMN_EM
            .max(header_minimum)
            .max(body_width)
            .max(unbreakable_width);
    }

    required_content_width + column_count as f64 * COLUMN_PADDING_EM > PORTRAIT_TABLE_WIDTH_EM
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

fn analyze_table(rows: &[Vec<String>]) -> Vec<ColumnLayout> {
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
    for row in rows.iter().skip(1) {
        for (index, value) in row.iter().take(column_count).enumerate() {
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

    stats
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let numeric = column.count > 0
                && column.numeric_count as f64 / column.count as f64 >= NUMERIC_RATIO_THRESHOLD;
            let alignment = if numeric {
                ColumnAlignment::Center
            } else if column.has_punctuation || column.has_long_text {
                ColumnAlignment::Left
            } else {
                ColumnAlignment::Center
            };
            let width = if column.count > 0 && column.all_short_digits {
                ColumnWidth::FixedEm(NARROW_NUMERIC_WIDTH_EM)
            } else {
                let ratio = (weights[index] / total_weight * column_count as f64)
                    .clamp(MIN_WIDTH_RATIO, MAX_WIDTH_RATIO);
                ColumnWidth::Relative((ratio * 10.0 + 0.5).floor() / 10.0)
            };
            ColumnLayout { alignment, width }
        })
        .collect()
}

pub(super) fn to_docx_grid(
    rows: &[Vec<String>],
    total_width_twips: usize,
    em_width_twips: usize,
) -> (Vec<usize>, Vec<ColumnAlignment>) {
    let columns = analyze_table(rows);
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

pub(super) fn to_longtblr(rows: &[Vec<String>]) -> String {
    let columns = analyze_table(rows);
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
            };
            match column.width {
                ColumnWidth::FixedEm(em) => format!("Q[{align},wd={em:.0}em]"),
                ColumnWidth::Relative(1.0) => format!("X[{align}]"),
                ColumnWidth::Relative(ratio) => format!("X[{ratio:.1},{align}]"),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let mut output = format!(
        "\\begin{{longtblr}}[\n  label = none,\n  entry = none,\n]{{\n  colspec = {{{colspec}}},\n  rowhead = 1,\n  hlines,\n  vlines,\n  row{{1}} = {{c, font=\\heiti\\enheiti}},\n}}\n"
    );
    for (row_index, row) in rows.iter().enumerate() {
        let cells = row
            .iter()
            .enumerate()
            .map(|(column_index, cell)| {
                let segments = inline_segments(cell);
                let cleaned = segments
                    .iter()
                    .map(|segment| segment.text.as_str())
                    .collect::<String>();
                let escaped = if name_column == Some(column_index) && row_index > 0 {
                    let name = latex_name(&cleaned);
                    if segments.iter().any(|segment| segment.bold) {
                        format!("\\textbf{{{name}}}")
                    } else {
                        name
                    }
                } else if row_index == 0 {
                    tex_escape(&cleaned)
                } else {
                    segments
                        .iter()
                        .map(|segment| {
                            let escaped = tex_escape(&segment.text);
                            if segment.bold {
                                format!("\\textbf{{{escaped}}}")
                            } else {
                                escaped
                            }
                        })
                        .collect::<String>()
                };
                if row_index == 0 {
                    format!("\\heiti\\enheiti {escaped}")
                } else {
                    escaped
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
        let (grid, _) = to_docx_grid(&rows(), 8_844, 280);
        assert_eq!(grid[0], 560);
        assert_eq!(grid.iter().sum::<usize>(), 8_844);
        assert!(grid[2] > grid[1]);
    }

    #[test]
    fn tex_uses_longtblr_with_matching_smart_columns() {
        let tex = to_longtblr(&rows());
        assert!(tex.contains("\\begin{longtblr}"));
        assert!(tex.contains("label = none"));
        assert!(tex.contains("entry = none"));
        assert!(!tex.contains("caption ="));
        assert!(tex.contains("Q[c,wd=2em]"));
        assert!(tex.contains("X["));
        assert!(tex.contains("rowhead = 1"));
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
        assert!(!requires_landscape(&compact));

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
        assert!(requires_landscape(&crowded));
    }

    #[test]
    fn many_rows_alone_do_not_trigger_landscape() {
        let mut table = vec![vec!["序号".into(), "名称".into()]];
        for index in 1..=100 {
            table.push(vec![index.to_string(), "短项".into()]);
        }
        assert!(!requires_landscape(&table));
    }

    #[test]
    fn tex_table_preserves_markdown_bold_and_normalizes_quotes() {
        let table = vec![
            vec!["项目".into(), "说明".into()],
            vec!["甲".into(), "\"**重点**\"内容".into()],
        ];
        let tex = to_longtblr(&table);
        assert!(tex.contains("“\\textbf{重点}”内容"), "{tex}");
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
        let tex = to_longtblr(&table);
        // 2 字姓名中间加 1em。
        assert!(tex.contains("张\\hspace{1em}三"));
        // 4 字姓名压缩到 3 字宽。
        assert!(tex.contains("\\resizebox{3em}{0.9em}{欧阳翠花}"));
        // 3 字姓名原样。
        assert!(tex.contains("王小明"));
    }
}
