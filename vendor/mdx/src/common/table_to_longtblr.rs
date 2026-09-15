//! 表格 → longtblr 转换。
//!
//! 智能表格处理：自动分析内容决定对齐方式和列宽分配。

use crate::common::ast::{Block, Inline};
use crate::common::table_layout::{analyze_table, ColumnLayout, ColumnWidth};

/// 生成智能列规格
fn generate_smart_colspec(columns: &[ColumnLayout]) -> String {
    columns
        .iter()
        .map(|column| {
            let align = column.alignment.latex();
            match column.width {
                ColumnWidth::FixedEm(em) => format!("Q[{},wd={}em]", align, em),
                ColumnWidth::Relative(ratio) => {
                    if ratio == 1.0 {
                        format!("X[{}]", align)
                    } else {
                        format!("X[{:.1},{}]", ratio, align)
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 转义 LaTeX 特殊字符（不处理行内格式标记）。
fn escape_latex(s: &str) -> String {
    let mut result = s.to_string();
    let latex_escapes = [
        ('\\', "\\textbackslash{}"),
        ('&', "\\&"),
        ('%', "\\%"),
        ('$', "\\$"),
        ('#', "\\#"),
        ('_', "\\_"),
        ('{', "\\{"),
        ('}', "\\}"),
        ('~', "\\textasciitilde{}"),
        ('^', "\\textasciicircum{}"),
    ];
    for (from, to) in latex_escapes {
        result = result.replace(from, to);
    }
    result
}

/// 将单元格内容转换为 LaTeX：先解析 **加粗**/*斜体*/`代码`/链接 行内格式，
/// 再对纯文本段落做 LaTeX 转义。
fn cell_to_latex(cell: &str) -> String {
    let mut result = String::new();
    push_cell_inlines(&mut result, &crate::common::inline::parse(cell));
    result
}

/// 把 inline::parse 之后的节点序列推入 result（处理粗体 / 斜体嵌套）。
/// 末尾返回借用以方便链式调用时复用，但本文件内主要靠 result 累加。
fn push_cell_inlines(result: &mut String, inlines: &[Inline]) {
    for ip in inlines {
        match ip {
            Inline::Text(t) => result.push_str(&escape_latex(t)),
            Inline::Bold(children) => {
                result.push_str("\\textbf{");
                push_cell_inlines(result, children);
                result.push('}');
            }
            Inline::Italic(children) => {
                result.push_str("\\textit{");
                push_cell_inlines(result, children);
                result.push('}');
            }
            Inline::Code(t) => {
                result.push_str("\\texttt{");
                result.push_str(&escape_latex(t));
                result.push('}');
            }
            Inline::Link { text, .. } => result.push_str(&escape_latex(text)),
            // longtblr 单元格内不插图，降级为替代文本
            Inline::Image { alt, .. } => result.push_str(&escape_latex(alt)),
            // 单元格内交叉引用：\ref 在 longtblr 中可用，直接输出
            Inline::CrossRef(id) => {
                result.push_str("\\ref{");
                result.push_str(id);
                result.push('}');
            }
            Inline::Citation(keys) => {
                result.push_str("\\cite{");
                result.push_str(&keys.join(","));
                result.push('}');
            }
            // longtblr 单元格内 \footnote 不生效，降级为全角括号内联注释
            Inline::Footnote(t) => {
                result.push('（');
                result.push_str(&escape_latex(t));
                result.push('）');
            }
        }
    }
}

/// 处理表格行
fn process_row(cells: &[String], is_header: bool) -> String {
    let rendered: Vec<String> = cells
        .iter()
        .map(|cell| {
            let content = cell_to_latex(cell);
            if is_header {
                format!("\\heiti {}", content)
            } else {
                content
            }
        })
        .collect();

    rendered.join(" & ") + " \\\\"
}

/// 生成 longtblr 环境的完整代码
/// 输入：表格行数据 Vec<Vec<String>>，第一行是表头
/// `label` 为交叉引用锚点（tabularray 外层 `label=` 选项）；引用表格需同时提供 caption。
pub fn emit_longtblr(rows: &[Vec<String>], caption: Option<&str>, label: Option<&str>) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let columns = analyze_table(rows);
    if columns.is_empty() {
        return String::new();
    }

    // 生成智能列规格
    let colspec = generate_smart_colspec(&columns);

    // 生成调试信息
    let mut debug_info = String::from("% 智能表格分析结果:\n");
    for (i, column) in columns.iter().enumerate() {
        let align_str = if column.alignment.latex() == 'c' {
            "居中"
        } else {
            "靠左"
        };
        debug_info.push_str(&format!(
            "%% 列{}: 平均宽度={:.1}, 最大宽度={:.1}, 数字列={}, 有标点={} → 对齐={}{}\n",
            i + 1,
            column.avg_display_width,
            column.max_display_width,
            if column.is_numeric { "是" } else { "否" },
            if column.has_punctuation { "是" } else { "否" },
            align_str,
            match column.width {
                ColumnWidth::FixedEm(em) => format!(", 窄数字列 → 固定列宽={}em", em),
                ColumnWidth::Relative(_) => String::new(),
            }
        ));
    }

    // 生成 longtblr 开始标记（外层选项按需含 caption 与 label）
    let mut outer_opts = String::new();
    if let Some(cap) = caption {
        outer_opts.push_str(&format!("caption={{{}}}", cap));
    }
    if let Some(lab) = label {
        if !outer_opts.is_empty() {
            outer_opts.push_str(", ");
        }
        outer_opts.push_str(&format!("label={{{}}}", lab));
    }
    let longtblr_begin = if outer_opts.is_empty() {
        format!(
            "{}\n\\begin{{longtblr}}{{\n  colspec = {{{}}},\n  rowhead = 1,\n  hlines,\n  vlines,\n  row{{1}} = {{c, font=\\heiti}},\n}}",
            debug_info, colspec
        )
    } else {
        format!(
            "{}\n\\begin{{longtblr}}[{}]{{\n  colspec = {{{}}},\n  rowhead = 1,\n  hlines,\n  vlines,\n  row{{1}} = {{c, font=\\heiti}},\n}}",
            debug_info, outer_opts, colspec
        )
    };

    let mut content_lines = vec![longtblr_begin];

    // 处理表头
    content_lines.push(process_row(&rows[0], true));

    // 处理表体
    for row in rows.iter().skip(1) {
        content_lines.push(process_row(row, false));
    }

    // 添加结束标记
    content_lines.push("\\end{longtblr}".to_string());

    content_lines.join("\n")
}

/// 将 Block::Table 转换为 longtblr LaTeX 代码
pub fn table_block_to_longtblr(block: &Block, caption: Option<&str>) -> String {
    match block {
        Block::Table {
            rows,
            caption: block_caption,
        } => emit_longtblr(rows, caption.or(block_caption.as_deref()), None),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_numeric_column_uses_fixed_width() {
        // 序号列：表头以外全是 2 位以内纯数字 → 固定 2em；其余列仍走 X 比例
        let rows = vec![
            vec!["序号".to_string(), "说明".to_string()],
            vec!["1".to_string(), "第一条说明文字，较长。".to_string()],
            vec!["12".to_string(), "第二条说明文字，同样较长。".to_string()],
        ];
        let result = emit_longtblr(&rows, None, None);
        assert!(result.contains("Q[c,wd=2em]"), "{result}");
        assert!(result.contains(" X["), "第二列应仍为 X 列: {result}");
    }

    #[test]
    fn three_digit_column_keeps_ratio_width() {
        // 出现 3 位数字即不算窄数字列，仍按原比例算法分配
        let rows = vec![
            vec!["数量".to_string(), "说明".to_string()],
            vec!["1".to_string(), "文字".to_string()],
            vec!["123".to_string(), "文字".to_string()],
        ];
        let result = emit_longtblr(&rows, None, None);
        assert!(!result.contains("wd=2em"), "{result}");
    }

    #[test]
    fn non_digit_content_is_not_narrow_numeric() {
        // 带单位/小数点的数字不算纯数字，不走固定列宽
        let rows = vec![
            vec!["比例".to_string(), "说明".to_string()],
            vec!["5%".to_string(), "文字".to_string()],
            vec!["1.5".to_string(), "文字".to_string()],
        ];
        let result = emit_longtblr(&rows, None, None);
        assert!(!result.contains("wd=2em"), "{result}");
    }

    #[test]
    fn narrow_numeric_ignores_header_and_empty_cells() {
        // 表头再宽也不影响判定；空单元格跳过
        let rows = vec![
            vec!["很长的表头文字说明".to_string(), "说明".to_string()],
            vec!["1".to_string(), "文字".to_string()],
            vec!["".to_string(), "文字".to_string()],
            vec!["99".to_string(), "文字".to_string()],
        ];
        let result = emit_longtblr(&rows, None, None);
        assert!(result.contains("Q[c,wd=2em]"), "{result}");
    }

    #[test]
    fn test_longtblr_output() {
        let rows = vec![
            vec!["列A".to_string(), "列B".to_string(), "列C".to_string()],
            vec!["1".to_string(), "2".to_string(), "文本内容".to_string()],
            vec!["3".to_string(), "4".to_string(), "更多文本".to_string()],
        ];
        let result = emit_longtblr(&rows, Some("测试表格"), None);
        assert!(result.contains("\\begin{longtblr}"));
        assert!(result.contains("\\end{longtblr}"));
        assert!(result.contains("caption={测试表格}"));
        assert!(!result.contains("label="));
    }

    #[test]
    fn test_longtblr_with_label() {
        let rows = vec![vec!["列A".to_string()], vec!["1".to_string()]];
        let result = emit_longtblr(&rows, Some("测试表格"), Some("tbl:products"));
        assert!(
            result.contains("caption={测试表格}, label={tbl:products}"),
            "{result}"
        );
    }

    #[test]
    fn table_cell_renders_citation() {
        let tex = emit_longtblr(&[vec!["文献".into()], vec!["[@a; @b]".into()]], None, None);
        assert!(tex.contains("\\cite{a,b}"), "{tex}");
    }
}
