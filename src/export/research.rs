//! 研究报告导出：frontmatter、插图与参考文献装进临时目录，交给固定版本 mdx：
//! Word 由 mdx 的 research 转换器生成，PDF 由 mdx 整理排版数据、`export::typst::research`
//! 排版。

use crate::export::parse::{MarkdownBlock, TableSpan, table_span_at};
use crate::models::{DraftInput, NumberingConfig, ResearchMetadata};
use anyhow::{Context, Result};
use mdx::{ConvertRequest, DocumentStyle, OutputFormat};
use std::fs;
use std::path::{Path, PathBuf};

/// 生成包含封面元数据的完整 Markdown。正文始终与元数据分开维护，避免模型或正文
/// 编辑器改写密级、编号、单位和日期。
pub(crate) fn markdown_with_frontmatter(
    input: &DraftInput,
    markdown: &str,
    bibliography: Option<&str>,
    numbering: &NumberingConfig,
) -> String {
    let meta = &input.research;
    let mut lines = vec![
        "---".to_string(),
        format!("密级: {}", one_line(&meta.security)),
        format!("文件类型: {}", one_line(&meta.file_type)),
        format!("文件编号: {}", one_line(&meta.file_number)),
        format!("文件版本号: {}", one_line(&meta.version)),
        format!("撰写单位: {}", one_line(&meta.institution)),
        format!("撰写时间: {}", one_line(&meta.date)),
        format!("文件名称: {}", one_line(&input.title_hint)),
    ];
    if !meta.security_years.trim().is_empty() {
        lines.insert(2, format!("保密年限: {}", one_line(&meta.security_years)));
    }
    // 封面题名的分行由这里按公文标题的断行规矩算好（jieba 分词 + 本单位词表），
    // PDF、Word、预览三处断在同一个地方。一行放得下就不写。
    if let Some(title) = cover_title(input, markdown) {
        let title_lines = super::title::cover_title_lines(&title);
        if title_lines.len() > 1 {
            lines.push(format!("题名分行: {}", title_lines.join("｜")));
        }
    }
    // 封面的可选行：留空就不写，mdx 据此不排那一行。
    for (key, value) in [
        ("标识行", &meta.ident),
        ("署名", &meta.byline),
        ("外文原题", &meta.original_title),
    ] {
        if !value.trim().is_empty() {
            lines.push(format!("{key}: {}", one_line(value)));
        }
    }
    if let Some(path) = bibliography.filter(|path| !path.trim().is_empty()) {
        lines.push(format!("bibliography: {}", one_line(path)));
    }
    lines.push("---".to_string());
    lines.push(String::new());
    let body = write_numbered_tables(markdown.trim_start_matches('\u{feff}').trim(), numbering);
    lines.push(body.trim().to_string());
    lines.push(String::new());
    lines.join("\n")
}

/// 把序号表的编号写进交给 mdx 的源码。
///
/// 首列编号与分组行由 `parse::normalize_numbered_table` 按设置生成——公文导出、
/// 研究报告预览走的都是它。这里把它的结果原样写回 Markdown：编号写成文字，
/// 分组行写成 `| （一）标题 |||` 这样的整行合并，手写的 `^^` 纵向合并照旧保留。
/// mdx 按合并单元格排，只需认得标记行（分组行靠左），不必再懂编号规矩，
/// 纸面与预览因此出自同一份编号结果。标记行本身留着，mdx 读到后不落版面。
fn write_numbered_tables(markdown: &str, numbering: &NumberingConfig) -> String {
    let mut output = String::with_capacity(markdown.len());
    let mut copied = 0;
    for located in super::parse_markdown_located_with_numbering(markdown, numbering) {
        let MarkdownBlock::Table {
            rows,
            spans,
            numbered: true,
            ..
        } = &located.block
        else {
            continue;
        };
        // 分隔行照抄源码：列对齐的冒号写在那里。
        let Some(separator) = markdown[located.range.clone()].lines().nth(1) else {
            continue;
        };
        output.push_str(&markdown[copied..located.range.start]);
        output.push_str(&table_markdown(rows, spans, separator.trim()));
        copied = located.range.end;
    }
    output.push_str(&markdown[copied..]);
    output
}

/// 把网格与合并单元格写回 MultiMarkdown 表格：被横向合并的格子写成紧挨的
/// `|`，纵向合并的续行在锚点列写 `^^`。与 `parse::parse_table_cells` 互逆。
fn table_markdown(rows: &[Vec<String>], spans: &[TableSpan], separator: &str) -> String {
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for (row_index, row) in rows.iter().enumerate() {
        let mut line = String::from("|");
        for (column, cell) in row.iter().enumerate() {
            match table_span_at(spans, row_index, column) {
                Some(span) if span.column != column => line.push('|'),
                Some(span) if span.row != row_index => line.push_str(" ^^ |"),
                _ => {
                    line.push(' ');
                    line.push_str(cell);
                    line.push_str(" |");
                }
            }
        }
        lines.push(line);
        if row_index == 0 {
            lines.push(separator.to_string());
        }
    }
    lines.join("\n")
}

/// 封面实际印的题名：文档要素的「文件名称」优先，留空才取正文区段的 `#`，
/// 与 mdx 的 `cover.title.or(report_title)`、预览的封面同一口径。
pub(crate) fn cover_title(input: &DraftInput, markdown: &str) -> Option<String> {
    let hint = input.title_hint.trim();
    if !hint.is_empty() {
        return Some(one_line(hint));
    }
    super::research_report_titles(markdown)
        .first()
        .map(|line| super::crossref::split_label(line).0.trim().to_string())
        .filter(|title| !title.is_empty())
}

/// 生成研究报告的 Word：mdx research 转换器排封面、目录与正文，封面与 PDF
/// 模板同一张网格（见 `mdx::cover`）。
pub(crate) fn write_docx(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    numbering: &NumberingConfig,
) -> Result<()> {
    let source = ResearchSourceBundle::create(
        input,
        markdown,
        numbering,
        crate::mermaid::Format::Png,
        &crate::storage::config_dir()?,
    )?;
    mdx::convert(ConvertRequest {
        input: source.markdown.clone(),
        output: Some(path.to_path_buf()),
        format: OutputFormat::Docx,
        style: DocumentStyle::Research,
    })
    .with_context(|| format!("研究报告 Word 转换失败：{}", path.display()))?;
    Ok(())
}

/// 研究报告的 Markdown 源码正文：正文前补上由文档要素生成的 frontmatter，
/// 有参考文献时指向包内的 `references.bib`。导出的这份可以直接交给 mdx 再转一次。
pub(crate) fn markdown_source(
    input: &DraftInput,
    markdown: &str,
    has_bibliography: bool,
    numbering: &NumberingConfig,
) -> String {
    markdown_with_frontmatter(
        input,
        markdown,
        has_bibliography.then_some("references.bib"),
        numbering,
    )
}

/// 交给 mdx 的源码目录：带 frontmatter 的 Markdown、插图与参考文献，放在一个临时
/// 目录里，随值一起删掉。
pub(crate) struct ResearchSourceBundle {
    root: tempfile::TempDir,
    pub(crate) markdown: PathBuf,
}

impl ResearchSourceBundle {
    /// 插图、文献的解析基准（临时目录本身）。
    pub(crate) fn root(&self) -> &Path {
        self.root.path()
    }

    pub(crate) fn create(
        input: &DraftInput,
        markdown: &str,
        numbering: &NumberingConfig,
        format: crate::mermaid::Format,
        base_dir: &Path,
    ) -> Result<Self> {
        let root = tempfile::Builder::new()
            .prefix("gongwen-research-")
            .tempdir()
            .context("无法创建研究报告临时目录")?;
        let rendered =
            crate::mermaid::materialize(markdown, crate::mermaid::Style::Research, format)?;
        crate::images::copy_refs_from(base_dir, &rendered, root.path())?;
        let bibliography = copy_bibliography(&input.research, root.path())?;
        let document = markdown_with_frontmatter(
            input,
            &rendered,
            bibliography.as_deref().map(|_| "references.bib"),
            numbering,
        );
        let path = root.path().join("research.md");
        fs::write(&path, document)
            .with_context(|| format!("无法写入研究报告临时文件：{}", path.display()))?;
        Ok(Self {
            root,
            markdown: path,
        })
    }
}

fn copy_bibliography(meta: &ResearchMetadata, target_dir: &Path) -> Result<Option<PathBuf>> {
    if meta.bibliography_content.trim().is_empty() {
        return Ok(None);
    }
    let destination = target_dir.join("references.bib");
    fs::write(&destination, &meta.bibliography_content)
        .with_context(|| format!("无法写入研究报告参考文献：{}", destination.display()))?;
    Ok(Some(destination))
}

fn one_line(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TemplateKind;

    #[test]
    fn frontmatter_is_generated_from_locked_metadata() {
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        input.research.security = "秘密".into();
        input.research.security_years = "5年".into();
        let text = markdown_with_frontmatter(
            &input,
            "## 第一章\n\n正文",
            Some("references.bib"),
            &NumberingConfig::default(),
        );
        assert!(text.starts_with("---\n密级: 秘密\n保密年限: 5年\n"));
        assert!(text.contains("文件名称: 测试报告"));
        assert!(text.contains("bibliography: references.bib"));
        assert!(text.ends_with("## 第一章\n\n正文\n"));
    }

    /// 序号表交给 mdx 前把编号写进源码：首列编号、分组行写成整行合并，分组
    /// 编号跟设置走；标记行留给 mdx（它认得、不落版面），别的表格一字不动。
    #[test]
    fn numbered_tables_are_written_out_before_mdx() {
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        let markdown = "\
表：任务分工
<!-- [序号表] -->
| 序号 | 事项 | 单位 |
| :---: | --- | --- |
| 重点工作 |  |  |
| 7 | 编制计划 | 办公室 |
|  | ^^ | 财务处 |

| A | B |
| --- | --- |
| 大标题 |  |";
        let numbering = NumberingConfig {
            table_group: crate::models::HeadingNumbering::Chinese,
            ..NumberingConfig::default()
        };
        let text = markdown_with_frontmatter(&input, markdown, None, &numbering);
        assert!(
            text.contains(
                "\
表：任务分工
<!-- [序号表] -->
| 序号 | 事项 | 单位 |
| :---: | --- | --- |
| 一、重点工作 |||
| 1 | 编制计划 | 办公室 |
| 2 | ^^ | 财务处 |

| A | B |
| --- | --- |
| 大标题 |  |"
            ),
            "{text}"
        );
    }

    /// 写回去的表格再交给 mdx 解析，网格与合并单元格要与公文助手的解析一致。
    #[test]
    fn written_numbered_table_parses_the_same_in_mdx() {
        let markdown = "\
<!-- [序号表] -->
| 序号 | 事项 | 单位 |
| --- | --- | --- |
| （一）重点工作 |||
| 1 | 编制计划 | 办公室 |
|  | ^^ | 财务处 |";
        let numbering = NumberingConfig::default();
        let written = write_numbered_tables(markdown, &numbering);
        let blocks = crate::export::parse_markdown_with_numbering(markdown, &numbering);
        let MarkdownBlock::Table { rows, spans, .. } = &blocks[1] else {
            panic!("应当解析为表格：{blocks:?}");
        };
        let lines = written.lines().skip(1).collect::<Vec<_>>();
        let source_rows = lines
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != 1)
            .map(|(_, line)| *line)
            .collect::<Vec<_>>();
        let parsed = mdx::table::parse_cells(&source_rows);
        assert_eq!(&parsed.rows, rows);
        let mdx_spans = parsed
            .spans
            .iter()
            .map(|span| (span.row, span.column, span.row_span, span.column_span))
            .collect::<Vec<_>>();
        let ours = spans
            .iter()
            .map(|span| (span.row, span.column, span.row_span, span.column_span))
            .collect::<Vec<_>>();
        assert_eq!(mdx_spans, ours);
    }

    #[test]
    fn frontmatter_carries_the_optional_cover_lines_only_when_filled() {
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        let text = markdown_with_frontmatter(&input, "正文", None, &NumberingConfig::default());
        for key in ["标识行", "署名", "外文原题"] {
            assert!(!text.contains(key), "留空不写 {key}：{text}");
        }
        input.research.ident = "课题编号：ZT-2026-07".into();
        input.research.byline = "政务智能化专题课题组".into();
        input.research.original_title = "AI RMF 1.0".into();
        let text = markdown_with_frontmatter(&input, "正文", None, &NumberingConfig::default());
        assert!(text.contains("标识行: 课题编号：ZT-2026-07"), "{text}");
        assert!(text.contains("署名: 政务智能化专题课题组"), "{text}");
        assert!(text.contains("外文原题: AI RMF 1.0"), "{text}");
        assert!(!text.contains("题名分行"), "一行放得下就不写分行：{text}");

        input.title_hint = "全市一体化政务数据共享平台（二期）建设项目".into();
        let text = markdown_with_frontmatter(&input, "正文", None, &NumberingConfig::default());
        let line = text
            .lines()
            .find(|line| line.starts_with("题名分行: "))
            .expect("长题名应写出分行");
        let parts: Vec<&str> = line["题名分行: ".len()..].split('｜').collect();
        assert_eq!(parts.len(), 2, "{line}");
        assert_eq!(parts.concat(), input.title_hint);
    }

    /// 研究报告的 Word 由 mdx research 转换器生成，封面要素一个不少。
    /// 研究报告 Word 里的序号表：分组行整行合并（gridSpan）且靠左，`^^` 写成
    /// vMerge，编号写在首列，标记行不落到纸上。
    #[test]
    fn research_word_export_merges_numbered_table_cells() {
        use std::io::Read as _;
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("报告.docx");
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "序号表测试".into(),
            ..Default::default()
        };
        write_docx(
            &path,
            &input,
            "<!-- [正文] -->\n\n## 任务分工\n\n<!-- [序号表] -->\n| 序号 | 事项 | 单位 |\n| --- | --- | --- |\n| 重点工作 |  |  |\n|  | 编制计划 | 办公室 |\n|  | ^^ | 财务处 |",
            &NumberingConfig::default(),
        )
        .expect("研究报告 Word 应转换成功");
        let file = fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(!xml.contains("[序号表]"), "标记行不许落到纸上");
        let group = xml.find("（一）重点工作").expect("分组行要带分组编号");
        let group_cell = &xml[xml[..group].rfind("<w:tc>").unwrap()..group];
        assert!(
            group_cell.contains(r#"<w:gridSpan w:val="3" />"#),
            "{group_cell}"
        );
        assert!(
            group_cell.contains(r#"<w:jc w:val="left" />"#),
            "{group_cell}"
        );
        assert!(xml.contains(r#"<w:vMerge w:val="restart" />"#), "{xml}");
        assert!(xml.contains(r#"<w:vMerge w:val="continue" />"#), "{xml}");
    }

    /// 研究报告 Word 里的居中 / 居右区：整行居中、右对齐，不带首行缩进。
    #[test]
    fn research_word_export_aligns_marked_lines() {
        use std::io::Read as _;
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("报告.docx");
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "对齐测试".into(),
            ..Default::default()
        };
        write_docx(
            &path,
            &input,
            "<!-- [正文] -->\n\n## 结语\n\n<!-- [居中] -->\n居中一行\n<!-- [居右] -->\n居右一行\n\n正文恢复。",
            &NumberingConfig::default(),
        )
        .expect("研究报告 Word 应转换成功");
        let file = fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(!xml.contains("居中]"), "标记行不许落到纸上");
        let paragraph = |needle: &str| {
            let at = xml.find(needle).expect(needle);
            xml[xml[..at].rfind("<w:p ").unwrap()..at].to_string()
        };
        let center = paragraph("居中一行");
        assert!(center.contains(r#"<w:jc w:val="center" />"#), "{center}");
        assert!(!center.contains("w:firstLine"), "{center}");
        let right = paragraph("居右一行");
        assert!(right.contains(r#"<w:jc w:val="right" />"#), "{right}");
    }

    #[test]
    fn research_word_export_prints_the_cover() {
        use std::io::Read as _;
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("报告.docx");
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "全市政务数据共享平台建设项目".into(),
            ..Default::default()
        };
        input.research.file_type = "建设实施方案".into();
        input.research.ident = "项目编号：XM-2026-014".into();
        input.research.institution = "市大数据管理局".into();
        input.research.date = "2026年9月".into();
        write_docx(
            &path,
            &input,
            "<!-- [正文] -->\n\n## 研究背景\n\n正文。",
            &NumberingConfig::default(),
        )
        .expect("研究报告 Word 应转换成功");
        let file = fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        for expected in [
            "建设实施方案",
            "项目编号：XM-2026-014",
            "全市政务数据共享平台建设项目",
            "建设实施",
            "市大数据管理局",
            "二〇二六年九月",
            "w:framePr",
        ] {
            assert!(xml.contains(expected), "Word 封面缺少 {expected}");
        }
    }

    /// 研究报告 PDF 的排版数据（mdx `typst_research` 整理、公式未排）。
    fn typst_data(input: &DraftInput, markdown: &str) -> serde_json::Value {
        let dir = tempfile::tempdir().expect("临时目录");
        let source = ResearchSourceBundle::create(
            input,
            markdown,
            &NumberingConfig::default(),
            crate::mermaid::Format::Pdf,
            dir.path(),
        )
        .expect("源码目录");
        let doc = mdx::typst_research::build(&source.markdown).expect("研究报告应转换成功");
        serde_json::to_value(&doc).unwrap()
    }

    fn research_input() -> DraftInput {
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        input
    }

    /// 预览把 `{#id}`、`{@id}`、`[@key]` 和表题换成纸面编号，靠的是"排版数据认这
    /// 几种写法"这个前提（见 `preview::research`）。哪天写法变了，预览会一声不响
    /// 地照旧换、PDF 却把源码符号原样印出来——所以这里真跑一遍转换核对。
    #[test]
    fn typst_data_turns_the_marks_the_preview_resolves_into_label_ref_cite_and_caption() {
        let mut input = research_input();
        input.research.bibliography_content = "@article{wang2020,title={甲},author={王}}\n".into();
        let data = typst_data(
            &input,
            concat!(
                "<!-- [正文] -->\n\n",
                "## 研究背景 {#chap:bg}\n\n",
                "见第{@chap:bg}章、表{@tbl:t}与图{@fig:a}[@wang2020]。\n\n",
                "表：样本分布 {#tbl:t}\n\n",
                "| 项 | 数 |\n| --- | --- |\n| 甲 | 1 |\n\n",
                "![总体架构](images/a.png){#fig:a}\n",
            ),
        );
        let json = data.to_string();
        for expected in [
            // 锚点：章、表两种挂载点都得认。
            r#""label":"chap:bg""#,
            r#""label":"tbl:t""#,
            // 交叉引用与文献引用。
            r#""t":"ref","id":"chap:bg""#,
            r#""t":"cite","keys":["wang2020"]"#,
            // 表题进表格。
            r#""caption":[{"t":"s","v":"样本分布"}]"#,
        ] {
            assert!(json.contains(expected), "排版数据应有 {expected}：{json}");
        }
        // 源码符号一个都不该漏进排版数据——漏了就会原样印进 PDF。
        for raw in ["{#", "{@", "[@", "表：样本分布"] {
            assert!(!json.contains(raw), "源码符号“{raw}”不应残留：{json}");
        }
    }

    /// 目录只在写了 `<!-- [目录] -->` 时排，排在标记处。
    #[test]
    fn typst_data_places_the_toc_only_where_the_marker_is() {
        let kinds = |data: &serde_json::Value| {
            data["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b["k"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        let input = research_input();
        let without = typst_data(
            &input,
            "<!-- [摘要] -->\n\n摘要。\n\n<!-- [正文] -->\n\n## 研究背景\n\n正文。\n",
        );
        assert!(!kinds(&without).contains(&"toc".to_string()));

        let with = typst_data(
            &input,
            "<!-- [目录] -->\n\n<!-- [摘要] -->\n\n摘要。\n\n<!-- [正文] -->\n\n## 研究背景\n\n正文。\n",
        );
        let kinds = kinds(&with);
        assert_eq!(kinds.iter().filter(|k| *k == "toc").count(), 1, "{kinds:?}");
        let toc = kinds.iter().position(|k| k == "toc").unwrap();
        let abstract_at = kinds.iter().position(|k| k == "abstract").unwrap();
        assert!(toc < abstract_at, "目录应排在标记处，即摘要之前：{kinds:?}");
    }

    /// 正文区段的 `#` 是报告题名：不排进正文版面，也不占章号——随后的 `##`
    /// 仍是第一章。预览与导航都按这个口径排。
    #[test]
    fn typst_data_keeps_a_body_h1_off_the_page_and_out_of_the_chapter_count() {
        let data = typst_data(
            &research_input(),
            "<!-- [正文] -->\n\n# 某某问题研究报告\n\n## 研究背景\n\n正文。\n",
        );
        let blocks = data["blocks"].to_string();
        assert!(
            !blocks.contains("某某问题研究报告"),
            "报告题名不应排进正文版面：{blocks}"
        );
        assert!(
            data["cover"].to_string().contains("测试报告"),
            "封面题名应取文件名称"
        );
        let chapters: Vec<_> = data["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["k"] == "chapter")
            .collect();
        assert_eq!(chapters.len(), 1, "{blocks}");
        assert_eq!(chapters[0]["prefix"], "第1章");
        assert!(chapters[0]["text"].to_string().contains("研究背景"));
    }

    /// 文档要素的「文件名称」留空时，封面题名回退到正文区的 `#`——与预览的
    /// `preview::research::report_title` 同一口径。
    #[test]
    fn typst_data_falls_back_to_the_body_h1_for_the_cover_title() {
        let mut input = research_input();
        input.title_hint.clear();
        let data = typst_data(
            &input,
            "<!-- [正文] -->\n\n# 某某问题研究报告\n\n## 研究背景\n\n正文。\n",
        );
        assert!(
            data["cover"]["title"]
                .to_string()
                .contains("某某问题研究报告"),
            "封面题名应回退到正文 `#`：{}",
            data["cover"]
        );
    }

    /// 反斜杠转义要与预览一致：`\$`、`\*` 印成字面符号，不能漏出反斜杠。
    /// anydoc 导入的 Word 正文就会带这些转义。
    #[test]
    fn typst_data_honours_backslash_escapes_like_the_preview() {
        let data = typst_data(
            &research_input(),
            concat!(
                "<!-- [正文] -->\n\n",
                "## 研究背景\n\n",
                "单价 \\$20 与 \\$价格$ 不是公式，a \\*b 2 \\* 3。\n\n",
                "公式 $x^{2}$ 照常。\n",
            ),
        );
        let json = data["blocks"].to_string();
        assert!(
            json.contains("单价 $20 与 $价格$ 不是公式，a *b 2 * 3。"),
            "{json}"
        );
        assert!(json.contains(r#""t":"math","v":"x^{2}""#), "{json}");
    }
}
