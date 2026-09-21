//! 交叉引用检查：转换前校验 `{@id}` 引用与 `{#id}` 锚点定义是否匹配。
//!
//! 硬错误（停止转换）：
//! - `{@id}` 引用了未定义（或在当前样式下不生效）的锚点；
//! - 同一锚点重复定义；
//! - 锚点不会生效：`Block::Label` 未挂接到标题/表格、带锚点的图片没有
//!   替代文本（无 caption 则不输出 \label）、带锚点的图片未独占一段、
//!   official 样式下的章节锚点。
//!
//! 软警告（仅打印，不停止）：锚点已定义但未被任何 `{@id}` 引用。
//!
//! 已知限制：不模拟区段标记模式——摘要/版本变更记录/参考文献段内标题上的
//! 锚点会被 emitter 丢弃，但本检查仍视为已定义（此类引用最终由 LaTeX 报
//! undefined reference）。

use std::collections::HashMap;

use super::ast::{Block, Inline};

/// 锚点在输出端的生效范围
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// research tex：章节/表格/图片锚点全部生效
    Full,
    /// official tex：图片/表格锚点生效（章节无自动编号，故标题锚点无效；
    /// 表格经 longtblr caption 获得自动编号，可引用）
    FiguresAndTables,
}

#[derive(Debug, Default)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// 校验 blocks 中的锚点定义与引用。
pub fn check(blocks: &[Block], support: Support) -> Report {
    let mut report = Report::default();
    // 锚点 id → 定义次数（重复定义报错）
    let mut defined: HashMap<String, usize> = HashMap::new();
    // 引用 id → 引用次数
    let mut used: HashMap<String, usize> = HashMap::new();

    macro_rules! define {
        ($id:expr) => {{
            let count = defined.entry($id.to_string()).or_insert(0);
            *count += 1;
            if *count > 1 {
                report
                    .errors
                    .push(format!("锚点 '{}' 重复定义（第 {} 次）", $id, *count));
            }
        }};
    }

    for (idx, block) in blocks.iter().enumerate() {
        match block {
            Block::Label(id) => {
                // 锚点必须挂接到紧随其后的标题或表格（跳过空行块）
                let next = blocks[idx + 1..]
                    .iter()
                    .find(|b| !matches!(b, Block::Empty));
                match next {
                    Some(Block::Heading { .. }) => {
                        if support == Support::FiguresAndTables {
                            report.errors.push(format!(
                                "锚点 '{id}' 在 official 样式下不生效（章节无自动编号，仅图片/表格锚点可用）"
                            ));
                        } else {
                            define!(id);
                        }
                    }
                    Some(Block::Table { .. }) => {
                        define!(id);
                    }
                    _ => {
                        report
                            .errors
                            .push(format!("锚点 '{id}' 未挂接到标题或表格，不会生效"));
                    }
                }
            }
            Block::Paragraph(inlines) => {
                collect_inline_refs(inlines, &mut used);
                check_image_labels(inlines, &mut report, &mut defined);
            }
            Block::List { content, .. } => {
                collect_inline_refs(content, &mut used);
                check_image_labels(content, &mut report, &mut defined);
            }
            Block::Table { rows, .. } => {
                // 单元格是 raw 字符串，引用在行内解析时才出现，此处补查
                for row in rows {
                    for cell in row {
                        for ip in super::inline::parse(cell) {
                            if let Inline::CrossRef(id) = ip {
                                *used.entry(id).or_insert(0) += 1;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // 引用未定义（或不生效）的锚点
    let mut dangling: Vec<&String> = used
        .keys()
        .filter(|id| !defined.contains_key(*id))
        .collect();
    dangling.sort();
    for id in dangling {
        report
            .errors
            .push(format!("'{{@{id}}}' 引用了未定义的锚点 '{id}'"));
    }

    // 已定义但未被引用：软警告
    let mut unused: Vec<&String> = defined
        .keys()
        .filter(|id| !used.contains_key(*id))
        .collect();
    unused.sort();
    for id in unused {
        report
            .warnings
            .push(format!("锚点 '{id}' 已定义但未被引用"));
    }

    report
}

/// 检查并按要求停止：警告照常打印，存在硬错误时逐条列出并中止转换。
pub fn check_or_bail(blocks: &[Block], support: Support) -> anyhow::Result<()> {
    let report = check(blocks, support);
    for w in &report.warnings {
        println!("  警告: {w}");
    }
    if !report.errors.is_empty() {
        for e in &report.errors {
            eprintln!("  交叉引用错误: {e}");
        }
        anyhow::bail!(
            "交叉引用检查未通过（{} 个错误），已停止转换",
            report.errors.len()
        );
    }
    Ok(())
}

fn collect_inline_refs(inlines: &[Inline], used: &mut HashMap<String, usize>) {
    for ip in inlines {
        if let Inline::CrossRef(id) = ip {
            *used.entry(id.clone()).or_insert(0) += 1;
        }
    }
}

/// 图片锚点只在"独占一段且有替代文本（caption）"时才生效，其余情况报硬错误。
fn check_image_labels(
    inlines: &[Inline],
    report: &mut Report,
    defined: &mut HashMap<String, usize>,
) {
    let meaningful: Vec<&Inline> = inlines
        .iter()
        .filter(|ip| !matches!(ip, Inline::Text(t) if t.trim().is_empty()))
        .collect();

    if let [Inline::Image { alt, label, .. }] = meaningful.as_slice() {
        if let Some(id) = label {
            if alt.is_empty() {
                report.errors.push(format!(
                    "图片锚点 '{id}' 不会生效：图片缺少替代文本（无 caption，不输出 \\label）"
                ));
            } else {
                let count = defined.entry(id.clone()).or_insert(0);
                *count += 1;
                if *count > 1 {
                    report
                        .errors
                        .push(format!("锚点 '{id}' 重复定义（第 {count} 次）"));
                }
            }
        }
        return;
    }

    for ip in meaningful {
        if let Inline::Image {
            label: Some(id), ..
        } = ip
        {
            report.errors.push(format!(
                "图片锚点 '{id}' 不会生效：图片未独占一段（无 figure/caption，不输出 \\label）"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn errors(md: &str, support: Support) -> Vec<String> {
        let blocks = parser::parse(md);
        check(&blocks, support).errors
    }

    #[test]
    fn valid_chapter_figure_table_refs_pass() {
        let md = "## 概述 {#chap:a}\n\n见第{@chap:a}章、图{@fig:x}、表{@tbl:t}。\n\n![图](a.png){#fig:x}\n\n表：题 {#tbl:t}\n\n| A |\n|---|\n| 1 |\n";
        let errs = errors(md, Support::Full);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn dangling_ref_is_error() {
        let errs = errors("见第{@chap:missing}章。", Support::Full);
        assert!(errs.iter().any(|e| e.contains("chap:missing")), "{errs:?}");
    }

    #[test]
    fn duplicate_label_is_error() {
        let errs = errors("## 甲 {#chap:x}\n\n## 乙 {#chap:x}\n", Support::Full);
        assert!(
            errs.iter()
                .any(|e| e.contains("重复定义") && e.contains("chap:x")),
            "{errs:?}"
        );
    }

    #[test]
    fn image_label_without_alt_is_error() {
        let errs = errors("![](a.png){#fig:x}\n\n见{@fig:x}。\n", Support::Full);
        assert!(
            errs.iter()
                .any(|e| e.contains("fig:x") && e.contains("替代文本")),
            "{errs:?}"
        );
    }

    #[test]
    fn inline_image_label_is_error() {
        let errs = errors(
            "文字 ![图](a.png){#fig:x} 混排\n\n见{@fig:x}。\n",
            Support::Full,
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("fig:x") && e.contains("未独占一段")),
            "{errs:?}"
        );
    }

    #[test]
    fn official_heading_label_is_error() {
        let errs = errors(
            "## 一、节 {#sec:a}\n\n见{@sec:a}。\n",
            Support::FiguresAndTables,
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("sec:a") && e.contains("official")),
            "{errs:?}"
        );
    }

    #[test]
    fn official_figure_label_passes() {
        let md = "![图](a.png){#fig:x}\n\n见{@fig:x}。\n";
        let errs = errors(md, Support::FiguresAndTables);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn official_table_label_passes() {
        let md = "表：产品清单 {#tbl:t}\n\n| A |\n|---|\n| 1 |\n\n见表{@tbl:t}。\n";
        let errs = errors(md, Support::FiguresAndTables);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn crossref_inside_table_cell_checked() {
        let md = "表：题 {#tbl:t}\n\n| A |\n|---|\n| 见{@fig:nope} |\n";
        let errs = errors(md, Support::Full);
        assert!(errs.iter().any(|e| e.contains("fig:nope")), "{errs:?}");
    }

    #[test]
    fn unused_label_is_warning_not_error() {
        let blocks = parser::parse("## 概述 {#chap:a}\n");
        let report = check(&blocks, Support::Full);
        assert!(report.is_ok());
        assert!(report.warnings.iter().any(|w| w.contains("chap:a")));
    }
}
