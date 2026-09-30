//! 导出前的解析结构快照；手工核对后保存，不能用更新快照掩盖规则变化。

use super::*;

#[test]
fn official_document_blocks_keep_heading_list_table_and_attachment_structure() {
    let markdown = "# 关于开展检查的通知\n\n## 一、工作安排\n\n1. 核对资料。\n2. 汇总情况。\n\n| 单位 | 数量 |\n| --- | ---: |\n| 甲单位 | 3 |\n\n<!-- [附件] -->\n\n# 检查清单\n\n附件正文。\n";
    insta::assert_debug_snapshot!("official_document_structure", parse_markdown(markdown));
}

#[test]
fn research_document_blocks_keep_formula_quote_image_and_diagram_structure() {
    let markdown = "# 研究报告\n\n## 方法\n\n行内公式 $a+b$ 保留。\n\n$$\nx^2+y^2=z^2\n$$\n\n> 引文第一行。\n> ——资料来源\n\n![示意图](images/example.png)\n\n```mermaid\nflowchart LR\n A[登记] --> B[办理]\n```\n";
    insta::assert_debug_snapshot!(
        "research_document_structure",
        parse_markdown_located_research(
            markdown,
            &ResearchMarks::default(),
            &NumberingConfig::default()
        )
    );
}
