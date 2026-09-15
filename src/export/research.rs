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
    if let Some(path) = bibliography.filter(|path| !path.trim().is_empty()) {
        lines.push(format!("bibliography: {}", one_line(path)));
    }
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(markdown.trim_start_matches('\u{feff}').trim().to_string());
    lines.push(String::new());
    lines.join("\n")
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
}
