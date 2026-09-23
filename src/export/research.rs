//! 研究报告导出：复用固定版本 mdx 的 research 转换器，PDF 统一交给本应用
//! 的内置 Tectonic 编译。

use crate::models::{DraftInput, ResearchMetadata};
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
    lines.push(markdown.trim_start_matches('\u{feff}').trim().to_string());
    lines.push(String::new());
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

/// 生成 mdx research 模式规定的主 TeX、类文件、分章、图片与参考文献文件。
pub(crate) fn write_tex(path: &Path, input: &DraftInput, markdown: &str) -> Result<()> {
    let source = ResearchSourceBundle::create(input, markdown)?;
    clear_generated_parts(path.parent().unwrap_or_else(|| Path::new(".")))?;
    mdx::convert(ConvertRequest {
        input: source.markdown.clone(),
        output: Some(path.to_path_buf()),
        format: OutputFormat::Tex,
        style: DocumentStyle::Research,
        template: None,
        // 编译只能使用 gongwen 固定的内置 Tectonic runtime。
        compile_pdf: false,
    })
    .with_context(|| format!("研究报告 TeX 转换失败：{}", path.display()))?;
    Ok(())
}

/// 生成研究报告的 Word：mdx research 转换器排封面、目录与正文，封面与 TeX
/// 模板同一张网格（见 `mdx::cover`）。
pub(crate) fn write_docx(path: &Path, input: &DraftInput, markdown: &str) -> Result<()> {
    let source = ResearchSourceBundle::create(input, markdown)?;
    mdx::convert(ConvertRequest {
        input: source.markdown.clone(),
        output: Some(path.to_path_buf()),
        format: OutputFormat::Docx,
        style: DocumentStyle::Research,
        template: None,
        compile_pdf: false,
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
) -> String {
    markdown_with_frontmatter(
        input,
        markdown,
        has_bibliography.then_some("references.bib"),
    )
}

/// 清掉上一次转换留下的分章、附录和图片目录。
///
/// mdx 只覆盖这一次用得上的文件，不删多余的。覆盖导出到同一个目录时，上一版多
/// 出来的 `data/chapter09.tex` 会留在那里：主 TeX 不 `\input` 它，编译看不出问题，
/// 但把这个目录打包发出去就多带了一章早已删掉的内容。
///
/// 只删这三个由 mdx 全权生成的目录，不碰导出目录里的其它东西。
fn clear_generated_parts(dir: &Path) -> Result<()> {
    for name in ["data", "appendix", "figures"] {
        let path = dir.join(name);
        if !path.is_dir() {
            continue;
        }
        fs::remove_dir_all(&path)
            .with_context(|| format!("无法清理上一次的研究报告产物：{}", path.display()))?;
    }
    Ok(())
}

struct ResearchSourceBundle {
    _root: tempfile::TempDir,
    markdown: PathBuf,
}

impl ResearchSourceBundle {
    fn create(input: &DraftInput, markdown: &str) -> Result<Self> {
        let root = tempfile::Builder::new()
            .prefix("gongwen-research-")
            .tempdir()
            .context("无法创建研究报告临时目录")?;
        crate::images::copy_refs(markdown, root.path())?;
        let bibliography = copy_bibliography(&input.research, root.path())?;
        let document = markdown_with_frontmatter(
            input,
            markdown,
            bibliography.as_deref().map(|_| "references.bib"),
        );
        let path = root.path().join("research.md");
        fs::write(&path, document)
            .with_context(|| format!("无法写入研究报告临时文件：{}", path.display()))?;
        Ok(Self {
            _root: root,
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
        let text = markdown_with_frontmatter(&input, "## 第一章\n\n正文", Some("references.bib"));
        assert!(text.starts_with("---\n密级: 秘密\n保密年限: 5年\n"));
        assert!(text.contains("文件名称: 测试报告"));
        assert!(text.contains("bibliography: references.bib"));
        assert!(text.ends_with("## 第一章\n\n正文\n"));
    }

    #[test]
    fn frontmatter_carries_the_optional_cover_lines_only_when_filled() {
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        let text = markdown_with_frontmatter(&input, "正文", None);
        for key in ["标识行", "署名", "外文原题"] {
            assert!(!text.contains(key), "留空不写 {key}：{text}");
        }
        input.research.ident = "课题编号：ZT-2026-07".into();
        input.research.byline = "政务智能化专题课题组".into();
        input.research.original_title = "AI RMF 1.0".into();
        let text = markdown_with_frontmatter(&input, "正文", None);
        assert!(text.contains("标识行: 课题编号：ZT-2026-07"), "{text}");
        assert!(text.contains("署名: 政务智能化专题课题组"), "{text}");
        assert!(text.contains("外文原题: AI RMF 1.0"), "{text}");
        assert!(!text.contains("题名分行"), "一行放得下就不写分行：{text}");

        input.title_hint = "全市一体化政务数据共享平台（二期）建设项目".into();
        let text = markdown_with_frontmatter(&input, "正文", None);
        let line = text
            .lines()
            .find(|line| line.starts_with("题名分行: "))
            .expect("长题名应写出分行");
        let parts: Vec<&str> = line["题名分行: ".len()..].split('｜').collect();
        assert_eq!(parts.len(), 2, "{line}");
        assert_eq!(parts.concat(), input.title_hint);
    }

    /// 研究报告的 Word 由 mdx research 转换器生成，封面要素一个不少。
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
        write_docx(&path, &input, "<!-- [正文] -->\n\n## 研究背景\n\n正文。")
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

    #[test]
    fn mdx_research_writes_the_expected_file_tree() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("报告.tex");
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        write_tex(
            &path,
            &input,
            "<!-- [正文] -->\n\n## 研究背景\n\n正文。\n\n<!-- [附录] -->\n\n## 数据表\n\n附录内容。",
        )
        .expect("研究报告应转换成功");
        assert!(path.is_file());
        assert!(dir.path().join("md2tex.cls").is_file());
        assert!(dir.path().join("data/chapter01.tex").is_file());
        assert!(dir.path().join("appendix/appendix01.tex").is_file());
    }

    /// 研究报告的字体必须全部来自随包分发的 runtime，不碰本机安装的字体。
    ///
    /// 这件事靠两边配合：mdx 的 md2tex.cls 认 `\MdxFontPath` 并按文件名加载，
    /// 本应用在编译前注入这个宏、并把 runtime 字体链进同一个目录。哪一边先变，
    /// 用户那边的表现都是"装了方正就好、没装就 ClassError"——很难查，所以在
    /// 这里把协议钉死。
    #[test]
    fn released_class_loads_research_fonts_from_the_injected_path() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("报告.tex");
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        write_tex(&path, &input, "## 研究背景\n\n正文。").expect("研究报告应转换成功");

        let class = fs::read_to_string(dir.path().join("md2tex.cls")).unwrap();
        assert!(
            class.contains("\\providecommand{\\MdxFontPath}{}"),
            "md2tex.cls 必须支持宿主注入的字体路径；mdx 升级后请同步本应用的注入逻辑"
        );
        // 路径分支引用的每个文件都必须真在 runtime 字体清单里，否则编译时
        // fontspec 才会报"找不到字体"。
        for file in crate::portable_runtime::RESEARCH_FONT_FILES {
            assert!(
                class.contains(file),
                "md2tex.cls 的字体 {file} 不在 RESEARCH_FONT_FILES 清单里"
            );
        }
        // ctex 的平台探测会让三个系统排出三种字形，必须钉死。
        assert!(
            class.contains("fontset=none"),
            "md2tex.cls 应钉死 ctex 字库"
        );
    }

    /// 预览把 `{#id}`、`{@id}`、`[@key]` 和表题换成纸面编号，靠的是"mdx 认这
    /// 几种写法"这个前提（见 `preview::research`）。哪天 mdx 换了写法，预览会
    /// 一声不响地照旧换、编译却把源码符号原样印进 PDF——所以这里真跑一遍转换，
    /// 拿生成的 TeX 核对。
    #[test]
    fn mdx_turns_the_marks_the_preview_resolves_into_label_ref_cite_and_caption() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("报告.tex");
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        input.research.bibliography_content = "@article{wang2020,title={甲},author={王}}\n".into();
        write_tex(
            &path,
            &input,
            concat!(
                "<!-- [正文] -->\n\n",
                "## 研究背景 {#chap:bg}\n\n",
                "见第{@chap:bg}章、表{@tbl:t}与图{@fig:a}[@wang2020]。\n\n",
                "表：样本分布 {#tbl:t}\n\n",
                "| 项 | 数 |\n| --- | --- |\n| 甲 | 1 |\n\n",
                "![总体架构](images/a.png){#fig:a}\n",
            ),
        )
        .expect("研究报告应转换成功");

        let tex = fs::read_to_string(&path).unwrap()
            + &fs::read_to_string(dir.path().join("data/chapter01.tex")).unwrap();
        for expected in [
            // 锚点：章、表、图三种挂载点都得认（表锚点是 longtblr 的外层选项）。
            "\\label{chap:bg}",
            "label={tbl:t}",
            "\\label{fig:a}",
            // 交叉引用与文献引用。
            "\\ref{chap:bg}",
            "\\cite{wang2020}",
            // 表题并进 longtblr 的 caption，图题进 figure 的 \caption。
            "caption={样本分布}",
            "\\caption{总体架构}",
        ] {
            assert!(
                tex.contains(expected),
                "mdx 应把这处标记转成 {expected}；预览的换算规则要跟着改：{tex}"
            );
        }
        // 源码符号一个都不该漏进 TeX——漏了就会原样印进 PDF。
        for raw in ["{#", "{@", "[@", "表：样本分布"] {
            assert!(
                !tex.contains(raw),
                "源码符号“{raw}”不应残留在 TeX 里：{tex}"
            );
        }
    }

    /// 正文区段的 `#` 是报告题名：mdx 不把它排进正文版面（封面已经印过一次，
    /// 再排一遍会在目录后多出一张只有一行标题的纸），也不占章号——随后的 `##`
    /// 仍是第一章。预览与导航都按这个口径排，这里真跑一遍转换核对。
    #[test]
    fn mdx_keeps_a_body_h1_off_the_page_and_out_of_the_chapter_count() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("报告.tex");
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        write_tex(
            &path,
            &input,
            concat!(
                "<!-- [正文] -->\n\n",
                "# 某某问题研究报告\n\n",
                "## 研究背景\n\n",
                "正文。\n",
            ),
        )
        .expect("研究报告应转换成功");

        let tex = fs::read_to_string(&path).unwrap()
            + &fs::read_to_string(dir.path().join("data/chapter01.tex")).unwrap();
        // 封面题名来自文档要素的「文件名称」（frontmatter），正文 `#` 不另排
        // 一份：整篇里题名只作为封面的 \papertitle 出现一次。
        assert_eq!(
            tex.matches("某某问题研究报告").count(),
            0,
            "报告题名不应排进正文版面：{tex}"
        );
        assert!(tex.contains("测试报告"), "封面题名应取文件名称：{tex}");
        assert_eq!(
            tex.matches("\\chapter").count(),
            1,
            "报告题名不应排成任何一种 \\chapter，编号章应只有“研究背景”一章：{tex}"
        );
        assert!(tex.contains("\\chapter{研究背景}"), "{tex}");
    }

    /// 文档要素的「文件名称」留空时，封面题名回退到正文区的 `#`——与预览的
    /// `preview::research::report_title` 同一口径，两边不会一个有题名一个空着。
    #[test]
    fn mdx_falls_back_to_the_body_h1_for_the_cover_title() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("报告.tex");
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: String::new(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        write_tex(
            &path,
            &input,
            "<!-- [正文] -->\n\n# 某某问题研究报告\n\n## 研究背景\n\n正文。\n",
        )
        .expect("研究报告应转换成功");

        let tex = fs::read_to_string(&path).unwrap();
        assert!(
            tex.contains("\\newcommand{\\papertitle}{某某问题研究报告}"),
            "封面题名应回退到正文 `#`：{tex}"
        );
    }
}
