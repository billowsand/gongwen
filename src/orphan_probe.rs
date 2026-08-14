//! 孤行探针回读：解析 TeX 编译产出的 `<jobname>.gwaproof`，把「哪一段末行只剩
//! 一两个字」精确报到审校面板。
//!
//! 为什么不靠 Rust 侧模拟宽度：TeX 是整段最优断行，字间胶有伸缩，同样 28 字宽的
//! 一行实测能装 27~30 个字符；标点还有挤压。纯字符宽度模拟只能给近似值
//! （`last_char_orphan` 就是那条近似路径，留给编辑时的即时提示）。这里读的是排版
//! 器自己吐出来的坐标，是事实而非估算。
//!
//! 文件由类文件的 `\GwaTail`（见 `gonghan-gwa.cls` 的“孤行审校探针”一节）写出，
//! 每段两行，字段之间以空白分隔：
//!
//! ```text
//! tail  <Markdown行号> <页码> <末行右缘sp> <字宽sp> <行宽sp> <奇数页左边界sp> <偶数页左边界sp>
//! lines <Markdown行号> <段落行数>
//! ```
//!
//! 每条 `tail` 自带当时的版心参数，附件页换了字号或版心也不会算错；duplex 时奇偶
//! 页左边界不同，按页码挑一个。坐标全部是 sp（1pt = 65536sp），相除即得“几个字位”。

/// 一段正文的实测排版结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ParagraphMetric {
    /// 段落在 Markdown 里的 1-based 起始行号，直接来自 `\GwaTail` 的参数。
    pub source_line: usize,
    /// 段落占用的行数（TeX 的 `\prevgraf`，跨页段落也准）。
    pub lines: usize,
    /// 末行内容宽度，单位是“字位”（一个三号汉字的宽度）。
    pub tail_chars: f32,
    /// 整行能排多少字位，用于给提示补一句参照。
    pub line_chars: f32,
}

/// 末行不足这么多字位就算孤行。
///
/// 探针前有 `\unskip`，量到的是剔除段末标点可压缩尾隙之后的净宽，比肉眼看到的
/// 右缘约小 0.7 字位（句号收尾时）。所以“2 个字 + 句号”实测约 2.3 字位，
/// “1 个字 + 句号”约 1.3 字位，取 2.5 正好卡在“末行只剩一两个字”这条线上。
pub const ORPHAN_THRESHOLD_CHARS: f32 = 2.5;

/// 解析 `.gwaproof` 的全部记录。无法识别的行直接跳过——探针文件是编译副产物，
/// 版本不匹配时宁可少报，不能让审校流程失败。
pub fn parse(report: &str) -> Vec<ParagraphMetric> {
    let mut tails: Vec<(usize, TailRecord)> = Vec::new();
    let mut lines: Vec<(usize, usize)> = Vec::new();
    for line in report.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("tail") => {
                let fields: Vec<i64> = parts.filter_map(|f| f.parse().ok()).collect();
                let [source_line, page, x, ccwd, hsize, odd_left, even_left] = fields[..] else {
                    continue;
                };
                if source_line <= 0 || ccwd <= 0 || hsize <= 0 {
                    continue;
                }
                let left = if page % 2 == 0 { even_left } else { odd_left };
                tails.push((
                    source_line as usize,
                    TailRecord {
                        tail_sp: x - left,
                        ccwd_sp: ccwd,
                        hsize_sp: hsize,
                    },
                ));
            }
            Some("lines") => {
                let fields: Vec<i64> = parts.filter_map(|f| f.parse().ok()).collect();
                let [source_line, count] = fields[..] else {
                    continue;
                };
                if source_line > 0 && count > 0 {
                    lines.push((source_line as usize, count as usize));
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for (source_line, tail) in tails {
        let Some((_, count)) = lines.iter().find(|(line, _)| *line == source_line) else {
            continue;
        };
        let line_chars = tail.hsize_sp as f32 / tail.ccwd_sp as f32;
        let tail_chars = tail.tail_sp as f32 / tail.ccwd_sp as f32;
        // 越界说明这一段的左边界不是按 oddsidemargin/evensidemargin 算的（横排附件、
        // 特殊盒子等）。这种记录宁可丢掉，也不要报一条算错的孤行。
        if !(0.0..=line_chars + 1.0).contains(&tail_chars) {
            continue;
        }
        out.push(ParagraphMetric {
            source_line,
            lines: *count,
            tail_chars,
            line_chars,
        });
    }
    out.sort_by_key(|metric| metric.source_line);
    out.dedup_by_key(|metric| metric.source_line);
    out
}

/// 从探针记录里挑出孤行段落。单行段落（如“特此函告。”）天然排除。
pub fn find_orphans(report: &str) -> Vec<ParagraphMetric> {
    parse(report)
        .into_iter()
        .filter(|metric| metric.lines >= 2 && metric.tail_chars <= ORPHAN_THRESHOLD_CHARS)
        .collect()
}

/// 组装审校提示。`markdown` 用来取那一段的开头几个字，让用户一眼认出是哪段。
pub fn format_warning(metric: &ParagraphMetric, markdown: &str) -> String {
    let preview = preview_of(markdown, metric.source_line);
    let mut message = format!(
        "第 {} 行段落排出来共 {} 行，末行只剩约 {:.1} 个字（整行 {:.0} 字），属于孤行",
        metric.source_line, metric.lines, metric.tail_chars, metric.line_chars
    );
    if !preview.is_empty() {
        message.push('：');
        message.push_str(&preview);
        message.push('…');
    }
    message.push_str("。请增删几个字把末行补足，或把内容并入上一行。");
    message
}

/// 取该行开头 24 个字符作为预览。
fn preview_of(markdown: &str, source_line: usize) -> String {
    markdown
        .lines()
        .nth(source_line.saturating_sub(1))
        .map(|line| line.trim().chars().take(24).collect())
        .unwrap_or_default()
}

struct TailRecord {
    tail_sp: i64,
    ccwd_sp: i64,
    hsize_sp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三号字宽 15.75bp ≈ 1036062sp；版心 28 字宽；左边界 28mm ≈ 5203238sp。
    const CCWD: i64 = 1_036_062;
    const HSIZE: i64 = CCWD * 28;
    const LEFT: i64 = 5_203_238;

    fn report(entries: &[(usize, usize, f32)]) -> String {
        entries
            .iter()
            .map(|(line, count, tail_chars)| {
                let x = LEFT + (*tail_chars * CCWD as f32) as i64;
                format!(
                    "tail {line} 1 {x} {CCWD} {HSIZE} {LEFT} {LEFT}\nlines {line} {count}",
                    line = line,
                    count = count,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_tail_and_line_records() {
        let metrics = parse(&report(&[(13, 4, 1.3)]));
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].source_line, 13);
        assert_eq!(metrics[0].lines, 4);
        assert!((metrics[0].tail_chars - 1.3).abs() < 0.01);
        assert!((metrics[0].line_chars - 28.0).abs() < 0.01);
    }

    #[test]
    fn flags_only_multi_line_short_tails() {
        let text = report(&[
            (5, 4, 1.3),   // 一个字加句号 → 报
            (9, 3, 2.3),   // 两个字加句号 → 报
            (17, 4, 12.0), // 末行够长 → 不报
            (21, 1, 0.8),  // 单行段落 → 不报
        ]);
        let flagged: Vec<usize> = find_orphans(&text)
            .into_iter()
            .map(|metric| metric.source_line)
            .collect();
        assert_eq!(flagged, vec![5, 9]);
    }

    #[test]
    fn skips_records_with_impossible_geometry() {
        // 左边界对不上的横排附件：末行算出来比整行还宽，必须丢弃而不是误报。
        let text = format!(
            "tail 7 1 {} {CCWD} {HSIZE} {LEFT} {LEFT}\nlines 7 3",
            LEFT + HSIZE * 3
        );
        assert!(parse(&text).is_empty());
    }

    #[test]
    fn picks_margin_by_page_parity() {
        // duplex 时偶数页左边界不同；用错边界会把末行算长 4 个字位。
        let even_left = LEFT + CCWD * 4;
        let x = even_left + CCWD; // 偶数页上末行实际只有 1 个字位
        let text = format!("tail 3 2 {x} {CCWD} {HSIZE} {LEFT} {even_left}\nlines 3 2");
        let metrics = parse(&text);
        assert_eq!(metrics.len(), 1);
        assert!((metrics[0].tail_chars - 1.0).abs() < 0.01);
    }

    #[test]
    fn tail_without_line_count_is_dropped() {
        let text = format!("tail 4 1 {} {CCWD} {HSIZE} {LEFT} {LEFT}", LEFT + CCWD);
        assert!(parse(&text).is_empty());
    }

    #[test]
    fn ignores_unknown_and_malformed_lines() {
        let text = format!(
            "rule whatever\ntail 4 1 not-a-number\n{}\n\n",
            report(&[(4, 2, 1.0)])
        );
        assert_eq!(parse(&text).len(), 1);
    }

    #[test]
    fn warning_names_the_line_and_quotes_the_text() {
        let markdown = "# 标题\n\n第一段。\n\n重点说明本单位服务事项的新增、取消、调整情况。";
        let metric = ParagraphMetric {
            source_line: 5,
            lines: 4,
            tail_chars: 1.3,
            line_chars: 28.0,
        };
        let text = format_warning(&metric, markdown);
        assert!(text.contains("第 5 行"));
        assert!(text.contains("共 4 行"));
        assert!(text.contains("1.3"));
        assert!(text.contains("重点说明本单位"));
    }
}
