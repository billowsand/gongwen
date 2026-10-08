//! PDF 的普通段落与列表复用预览解析器，避免 mdx 的一行一段和固定列表编号。
//! 只规范普通正文区；研究报告专有结构保留源码，继续由 mdx 解析。

use crate::export::{MarkdownBlock, parse_markdown_located_with_numbering, render_list_number};
use crate::models::NumberingConfig;
use mdx::typst_research::{LIST_LABEL_CLOSE, LIST_LABEL_OPEN};

pub(super) fn normalize(markdown: &str, numbering: &NumberingConfig) -> String {
    let mut output = String::new();
    let mut ordinary = String::new();
    let mut code = false;
    let mut math = false;
    let mut aligned = false;
    for raw in markdown.lines() {
        let line = raw.trim();
        if code || math || aligned {
            output.push_str(raw);
            output.push('\n');
            if code && line.starts_with("```") {
                code = false;
            } else if math && line.ends_with("$$") {
                math = false;
            } else if aligned && line.is_empty() {
                aligned = false;
            }
            continue;
        }
        // 表题独立保留：不能被并入前后段落，否则 mdx 无法挂到表格与锚点上。
        let structural = line.starts_with('#')
            || line.starts_with("```")
            || line.starts_with("$$")
            || line.starts_with('<')
            || line.starts_with("![")
            || mdx::quote::strip_marker(line).is_some()
            || mdx::table::is_table_line(line)
            || is_caption(line);
        if structural {
            flush(&mut ordinary, &mut output, numbering);
            output.push_str(raw);
            output.push('\n');
            code = line.starts_with("```");
            math = line.starts_with("$$") && (line == "$$" || !line[2..].ends_with("$$"));
            aligned = crate::export::parse::parse_align_marker(line).is_some();
        } else {
            ordinary.push_str(raw);
            ordinary.push('\n');
        }
    }
    flush(&mut ordinary, &mut output, numbering);
    output
}

fn is_caption(line: &str) -> bool {
    ["Table:", "table:", "TABLE:", "表:", "表：", ":"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn flush(ordinary: &mut String, output: &mut String, numbering: &NumberingConfig) {
    if ordinary.is_empty() {
        return;
    }
    for located in parse_markdown_located_with_numbering(ordinary, numbering) {
        match located.block {
            MarkdownBlock::Paragraph(mut text) => {
                // 可见字符范围由预览解析器计算，反投影回 Markdown 字节范围。
                // 从后往前插临时标记，不改正文、粗体、引用和公式的源码。
                let spans = crate::export::inline_char_spans(&text);
                for range in located.generated_prefixes.iter().rev() {
                    let start = spans[range.start].0.start;
                    let end = spans[range.end - 1].0.end;
                    text.insert(end, LIST_LABEL_CLOSE);
                    text.insert(start, LIST_LABEL_OPEN);
                }
                output.push_str(&text);
            }
            MarkdownBlock::OrderedListItem { number, text } => {
                let prefix = render_list_number(numbering.list2, number);
                // 写成字面编号：不可让 mdx 再解析为列表，重编号或改变成段方式。
                output.push(LIST_LABEL_OPEN);
                output.push_str(&prefix);
                output.push(LIST_LABEL_CLOSE);
                output.push_str(&text);
            }
            _ => unreachable!("普通正文区只应包含段落与列表"),
        }
        output.push_str("\n\n");
    }
    // 区段前后的空行仍是边界，尤其是表题、图片和对齐区。
    if ordinary.trim().is_empty() {
        output.push('\n');
    }
    ordinary.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ListNumbering;

    fn unmark(source: &str) -> String {
        source.replace([LIST_LABEL_OPEN, LIST_LABEL_CLOSE], "")
    }

    #[test]
    fn soft_lines_and_lists_follow_preview() {
        let source = "第一行\n第二行。\n\nEnglish\nwords.\n\n要求：\n3. **甲**；\n1. 乙。\n\n3. 独立甲\n1. 独立乙\n";
        assert_eq!(
            unmark(&normalize(source, &NumberingConfig::default())),
            "第一行第二行。\n\nEnglish words.\n\n要求：③**甲**；④乙。\n\n3.独立甲。\n\n4.独立乙。\n\n"
        );
        let numbering = NumberingConfig {
            list1: ListNumbering::HalfParen,
            list2: ListNumbering::FullParen,
            ..Default::default()
        };
        assert_eq!(
            unmark(&normalize("要求：\n- 甲\n- 乙\n\n- 丙\n- 丁", &numbering)),
            "要求：(1)甲。(2)乙。\n\n（1）丙。\n\n（2）丁。\n\n"
        );
    }

    #[test]
    fn research_structures_keep_their_source() {
        let source = concat!(
            "## 方法 {#chap:method}\n\n",
            "```rust\nlet a = 1;\n\nlet b = 2;\n```\n\n",
            "$$\na + b\n= c\n$$\n\n",
            "<!-- [居中] -->\n第一行\n第二行\n\n",
            "<!--【RIGHT】-->\n第一行\n第二行\n\n",
            "> [!案例] 标题\n> 第一段\n> 第二段\n\n",
            "表：数据 {#tbl:data}\n\n| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n\n",
            "![图片](images/test.png){#fig:test}\n"
        );
        assert_eq!(normalize(source, &NumberingConfig::default()), source);
    }

    #[test]
    fn pdf_data_keeps_soft_lines_inline_lists_and_empty_line_boundaries() {
        let source = "# 报告\n\n<!-- [摘要] -->\n\n摘要前半\n后半。\n\n<!-- [正文] -->\n\n## 方法\n\n短句前半\n后半。\n\n要求：\n- **甲**；\n- 乙含公式 $x=1$。\n\n3. 独立甲\n1. 独立乙\n\n1. 新组\n\n末段。";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.md");
        std::fs::write(&path, source).unwrap();
        let data = serde_json::to_value(
            mdx::typst_research::build_with_body_normalizer(&path, |body| {
                normalize(body, &NumberingConfig::default())
            })
            .unwrap(),
        )
        .unwrap();
        let blocks = data["blocks"].as_array().unwrap();
        let abstract_block = blocks.iter().find(|b| b["k"] == "abstract").unwrap();
        assert_eq!(abstract_block["blocks"][0]["c"][0]["v"], "摘要前半后半。");
        let paragraphs: Vec<_> = blocks.iter().filter(|b| b["k"] == "par").collect();
        assert_eq!(paragraphs.len(), 6);
        assert_eq!(paragraphs[0]["c"][0]["v"], "短句前半后半。");
        assert_eq!(paragraphs[1]["c"][0]["v"], "要求：");
        assert_eq!(paragraphs[1]["c"][1]["t"], "list-label");
        assert_eq!(paragraphs[1]["c"][1]["v"], "①");
        assert_eq!(paragraphs[1]["c"][2]["t"], "b");
        assert!(
            paragraphs[1]["c"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["t"] == "math")
        );
        for (block, label, text) in [
            (paragraphs[2], "3.", "独立甲。"),
            (paragraphs[3], "4.", "独立乙。"),
            (paragraphs[4], "1.", "新组。"),
        ] {
            assert_eq!(block["c"][0]["t"], "list-label");
            assert_eq!(block["c"][0]["v"], label);
            assert_eq!(block["c"][1]["v"], text);
        }
        assert_eq!(paragraphs[5]["c"][0]["v"], "末段。");
        assert!(blocks.iter().all(|b| b["k"] != "list"));
    }
}
