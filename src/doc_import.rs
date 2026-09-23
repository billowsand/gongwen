//! 从现有文档导入正文：把 Word / Excel / PowerPoint / OpenDocument / RTF /
//! EPUB / CSV 转成 GitHub 风味 markdown，供起草页在光标处插入或直接新建一篇稿件。
//!
//! 转换由 `anydoc` 完成——纯 Rust 实现，不依赖 Office、LibreOffice 或任何外部
//! 进程，符合本软件"离线可用"的前提。纯文本与 markdown 不走它，直接按 UTF-8 读。
//!
//! **PDF 有意不在支持之列**。PDF 里没有段落、标题、表格这些结构，抽出来的只是
//! 一堆按坐标排的文字片段：正文会被拼成一整段，表格会散架；中文 PDF 还得看
//! 字体有没有内嵌 ToUnicode 映射，没有就是满屏乱码；扫描件更是一个字都取不到。
//! 与其让人拿到一份需要重排的稿子，不如在选文件这一步就说清楚不支持。

use crate::models::ResearchMetadata;
use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};

/// 走 `anydoc` 转换的二进制文档格式，全部小写。
const CONVERTED: &[&str] = &[
    // Word
    "docx", "doc", "docm", //
    // Excel
    "xlsx", "xls", "xlsm", "xlsb", //
    // PowerPoint
    "pptx", "ppt", "pptm", "ppsx", "ppsm", "pps", "pot", //
    // OpenDocument
    "odt", "ods", "odp", //
    // 其余
    "rtf", "epub", "csv",
];

/// 本来就是文本，直接读进来即可，不必绕 `anydoc`。
const PLAIN: &[&str] = &["md", "markdown", "txt"];

/// 文件对话框里的分组过滤器。第一组是"所有支持的格式"，由下面几组拼出来。
const FILTER_GROUPS: &[(&str, &[&str])] = &[
    ("Word 文档", &["docx", "doc", "docm"]),
    ("Excel 工作簿", &["xlsx", "xls", "xlsm", "xlsb"]),
    (
        "PowerPoint 演示文稿",
        &["pptx", "ppt", "pptm", "ppsx", "ppsm", "pps", "pot"],
    ),
    ("OpenDocument", &["odt", "ods", "odp"]),
    ("RTF / EPUB / CSV", &["rtf", "epub", "csv"]),
    ("纯文本 / Markdown", &["md", "markdown", "txt"]),
];

/// 打开"从文档导入"的文件选择框，返回用户选中的文件。
pub(crate) fn pick_file() -> Option<std::path::PathBuf> {
    let all: Vec<&str> = FILTER_GROUPS
        .iter()
        .flat_map(|(_, extensions)| extensions.iter().copied())
        .collect();
    let mut dialog = rfd::FileDialog::new().add_filter("所有支持的格式", &all);
    for (label, extensions) in FILTER_GROUPS {
        dialog = dialog.add_filter(*label, extensions);
    }
    dialog.pick_file()
}

/// 打开“从文件夹新建研究报告”的目录选择框。
pub(crate) fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

#[derive(Debug)]
pub(crate) struct ResearchFolderImport {
    pub(crate) markdown: String,
    pub(crate) title: String,
    pub(crate) metadata: ResearchMetadata,
    pub(crate) file_names: Vec<String>,
    /// 没能入库的图片引用（文件缺失、格式不支持、路径越界）。正文里这些引用
    /// 原样保留，交给界面提示用户补齐。
    pub(crate) skipped_images: Vec<String>,
}

/// 按 mdx research 规则合并目录第一层的 Markdown，并把图片与 BibTeX 变成
/// 应用自持有的稿件资源。导入完成后移动或删除原文件夹不影响后续导出。
pub(crate) fn import_research_folder(folder: &Path) -> Result<ResearchFolderImport> {
    if !folder.is_dir() {
        bail!("所选路径不是文件夹：{}", folder.display());
    }
    let mut files = std::fs::read_dir(folder)
        .with_context(|| format!("无法读取文件夹 {}", folder.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        let left = left
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let right = right
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        left.cmp(right)
    });
    if files.is_empty() {
        bail!("所选文件夹第一层没有 .md 文件");
    }

    let mut chunks = Vec::with_capacity(files.len());
    for (index, path) in files.iter().enumerate() {
        let content = crate::text_file::read_to_string(path)
            .with_context(|| format!("读取 {} 失败", file_label(path)))?;
        let content = content.trim_start_matches('﻿').to_string();
        if index > 0 && starts_with_frontmatter(&content) {
            bail!(
                "{} 包含第二份 frontmatter；只有排序第一的 Markdown 可以填写文档要素",
                file_label(path)
            );
        }
        chunks.push(content);
    }

    let merged = chunks.join("\n\n");
    let (mut metadata, frontmatter_title, bibliography, body) = parse_research_frontmatter(&merged);
    let heading_title = first_h1(&body);
    let title = frontmatter_title
        .or(heading_title)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| {
            folder
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "研究报告".to_string())
        });
    let body = remove_first_h1(&body);

    if let Some(relative) = bibliography {
        let path = safe_resource_path(folder, &relative)?;
        if !path.is_file() {
            bail!("frontmatter 指定的 BibTeX 文件不存在：{}", path.display());
        }
        metadata.bibliography_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "references.bib".to_string());
        metadata.bibliography_content = crate::text_file::read_to_string(&path)
            .with_context(|| format!("无法读取 BibTeX 文件 {}", path.display()))?;
    }

    let (markdown, skipped_images) = crate::images::import_referenced_from(body.trim(), folder)?;
    if markdown.trim().is_empty() {
        bail!("合并后的研究报告正文为空");
    }
    Ok(ResearchFolderImport {
        markdown,
        title,
        metadata,
        file_names: files
            .iter()
            .map(|path| file_label(path))
            .collect::<Vec<_>>(),
        skipped_images,
    })
}

/// 供界面提示用的格式清单。
pub(crate) fn supported_summary() -> String {
    FILTER_GROUPS
        .iter()
        .map(|(label, _)| *label)
        .collect::<Vec<_>>()
        .join("、")
}

/// 把一个文档转成 markdown。返回的内容已经归一化，可直接插进编辑器。
pub(crate) fn to_markdown(path: &Path) -> Result<String> {
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if extension == "pdf" {
        bail!(
            "PDF 不支持导入：PDF 只存版面不存结构，抽出来的正文会丢段落、表格会散架，\
             扫描件更是取不到文字。请改用原始的 Word 文件，或先另存为 docx。"
        );
    }
    let markdown = if PLAIN.contains(&extension.as_str()) {
        crate::text_file::read_to_string(path)
            .with_context(|| format!("读取 {} 失败", file_label(path)))?
    } else if CONVERTED.contains(&extension.as_str()) {
        anydoc::to_markdown(path)
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| format!("解析 {} 失败", file_label(path)))?
    } else {
        bail!(
            "不支持的文件格式 .{extension}：可导入 {}。",
            supported_summary()
        );
    };
    let markdown = normalize(&markdown);
    if markdown.is_empty() {
        bail!(
            "{} 里没有提取到文字：文件可能是空的，或者内容全是图片。",
            file_label(path)
        );
    }
    Ok(markdown)
}

/// 状态栏与错误信息里用的文件名。
pub(crate) fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// 归一化转换结果：统一换行、掐掉行尾空白、把连续空行压成一个，首尾留白去净。
///
/// 转换器对空行的处理各格式不一（表格后面常跟两三个空行），直接插进编辑器会在
/// 稿子中间留下大片空白，导出时还会被当成段落分隔。
fn normalize(markdown: &str) -> String {
    let unified = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = Vec::new();
    for line in unified.lines() {
        let line = line.trim_end();
        // 连续空行只留一个。
        if line.is_empty() && lines.last().is_some_and(|last: &&str| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

/// 判断一份 Markdown 是不是以 frontmatter 开头。
///
/// 光看"首行是 `---` 且后面还有 `---`"会误伤：分章文件开头写一条分隔线、正文里
/// 再写一条，就会被当成 frontmatter。所以还要求围栏里每一行都是 `键: 值` 或空行
/// ——分隔线之间夹的是正文，一眼就能区分。
fn starts_with_frontmatter(content: &str) -> bool {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return false;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            return true;
        }
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, _)) = trimmed.split_once([':', '：']) else {
            return false;
        };
        // 键里不该有空格或 Markdown 记号；`## 标题: 副题` 这种会被挡在这里。
        if key.trim().is_empty() || key.contains([' ', '#', '*', '-', '>']) {
            return false;
        }
    }
    false
}

fn parse_research_frontmatter(
    content: &str,
) -> (ResearchMetadata, Option<String>, Option<String>, String) {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (ResearchMetadata::default(), None, None, content.to_string());
    }
    let mut metadata = ResearchMetadata::default();
    let mut title = None;
    let mut bibliography = None;
    let mut consumed = 1usize;
    let mut closed = false;
    for line in lines {
        consumed += 1;
        let trimmed = line.trim();
        if trimmed == "---" {
            closed = true;
            break;
        }
        let Some((key, value)) = trimmed.split_once([':', '：']) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "密级" | "security" => metadata.security = value.into(),
            "年限" | "保密年限" | "保密期限" | "years" => {
                metadata.security_years = value.into()
            }
            "文件类型" | "类型" | "doctype" => metadata.file_type = value.into(),
            "文件编号" | "编号" | "number" => metadata.file_number = value.into(),
            "文件版本号" | "版本" | "version" => metadata.version = value.into(),
            "撰写单位" | "单位" | "institution" => metadata.institution = value.into(),
            "撰写时间" | "时间" | "日期" | "date" => metadata.date = value.into(),
            "标识行" | "期号" | "项目编号" | "课题编号" | "原文出处" | "ident" => {
                metadata.ident = value.into()
            }
            "署名" | "署名行" | "课题组" | "byline" => metadata.byline = value.into(),
            "外文原题" | "原文题名" | "original" => metadata.original_title = value.into(),
            "文件名称" | "标题" | "title" => title = Some(value.to_string()),
            "bibliography" => bibliography = Some(value.to_string()),
            _ => {}
        }
    }
    if !closed {
        return (ResearchMetadata::default(), None, None, content.to_string());
    }
    (
        metadata,
        title,
        bibliography,
        content
            .lines()
            .skip(consumed)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn first_h1(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        line.strip_prefix("# ")
            .map(|title| title.trim())
            .filter(|title| !title.is_empty())
            .map(|title| {
                let plain = title
                    .split_once(" {#")
                    .map(|(plain, _)| plain)
                    .unwrap_or(title)
                    .trim();
                // 手工编号要剥掉，与 mdx 取封面标题的行为一致：`# 一、测试报告`
                // 的封面题名是"测试报告"。这里不剥的话，编号会经由 frontmatter
                // 的 `文件名称:` 原样印到封面上——mdx 那边不会再清洗一次。
                crate::export::clean_heading_number(plain)
            })
            .filter(|title| !title.is_empty())
    })
}

fn remove_first_h1(content: &str) -> String {
    let mut removed = false;
    content
        .lines()
        .filter(|line| {
            if !removed && line.starts_with("# ") {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn safe_resource_path(base: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("研究报告资源必须位于所选文件夹内：{relative}");
    }
    Ok(base.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_collapses_blank_runs_and_trims() {
        let raw = "# 标题\r\n\r\n\r\n正文一   \r\n\r\n\r\n\r\n正文二\r\n\r\n";
        assert_eq!(normalize(raw), "# 标题\n\n正文一\n\n正文二");
    }

    #[test]
    fn normalize_keeps_single_blank_between_blocks() {
        let raw = "| a | b |\n| --- | --- |\n| 1 | 2 |\n\n结语";
        assert_eq!(normalize(raw), raw);
    }

    #[test]
    fn pdf_is_rejected_with_reason() {
        let error = to_markdown(&PathBuf::from("样例.pdf")).unwrap_err();
        assert!(format!("{error:#}").contains("PDF 不支持导入"));
    }

    #[test]
    fn unknown_extension_is_rejected() {
        let error = to_markdown(&PathBuf::from("样例.wps")).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("不支持的文件格式 .wps"), "{message}");
    }

    #[test]
    fn plain_text_is_read_directly() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("底稿.md");
        std::fs::write(&path, "# 关于X的通知\r\n\r\n\r\n正文  \r\n").expect("写入");
        assert_eq!(to_markdown(&path).unwrap(), "# 关于X的通知\n\n正文");
    }

    #[test]
    fn csv_becomes_a_markdown_table() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("台账.csv");
        std::fs::write(&path, "序号,事项\n1,材料报送\n").expect("写入");
        let markdown = to_markdown(&path).unwrap();
        assert!(markdown.contains("| 序号 | 事项 |"), "{markdown}");
        assert!(markdown.contains("| 1 | 材料报送 |"), "{markdown}");
    }

    /// 转换器自带的 calamine 与本软件词库导入用的是两个大版本，同时链进来会不会
    /// 出岔子只有真跑一遍才知道——这里现造一个 xlsx 再转回 markdown。
    #[test]
    fn xlsx_round_trips_through_the_converter() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("台账.xlsx");
        let mut workbook = rust_xlsxwriter::Workbook::new();
        let sheet = workbook.add_worksheet();
        for (row, cells) in [["序号", "事项"], ["1", "材料报送"]].iter().enumerate() {
            for (column, value) in cells.iter().enumerate() {
                sheet
                    .write_string(row as u32, column as u16, *value)
                    .expect("写单元格");
            }
        }
        workbook.save(&path).expect("保存 xlsx");
        let markdown = to_markdown(&path).unwrap();
        assert!(markdown.contains("材料报送"), "{markdown}");
    }

    #[test]
    fn empty_file_reports_no_text() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("空.txt");
        std::fs::write(&path, "   \n\n").expect("写入");
        let error = to_markdown(&path).unwrap_err();
        assert!(format!("{error:#}").contains("没有提取到文字"));
    }

    #[test]
    fn research_folder_merges_top_level_markdown_by_file_name() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("02-分析.md"), "## 分析\n\n第二部分。").unwrap();
        std::fs::write(
            dir.path().join("01-概述.md"),
            "---\n密级: 内部\n文件名称: 测试研究\n撰写单位: 测试单位\n---\n# 会被移除的标题\n\n## 概述\n\n第一部分。",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("子目录")).unwrap();
        std::fs::write(dir.path().join("子目录/00-忽略.md"), "## 不应导入").unwrap();

        let imported = import_research_folder(dir.path()).unwrap();
        assert_eq!(imported.title, "测试研究");
        assert_eq!(imported.metadata.security, "内部");
        assert_eq!(imported.file_names, ["01-概述.md", "02-分析.md"]);
        assert!(!imported.markdown.contains("# 会被移除的标题"));
        assert!(
            imported.markdown.find("## 概述").unwrap() < imported.markdown.find("## 分析").unwrap()
        );
        assert!(!imported.markdown.contains("不应导入"));
    }

    #[test]
    fn research_folder_imports_bibliography_into_snapshot() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(
            dir.path().join("01.md"),
            "---\nbibliography: refs/library.bib\n---\n## 正文\n\n引用 [@demo]。",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("refs")).unwrap();
        std::fs::write(
            dir.path().join("refs/library.bib"),
            "@book{demo, title={示例}}",
        )
        .unwrap();

        let imported = import_research_folder(dir.path()).unwrap();
        assert_eq!(imported.metadata.bibliography_name, "library.bib");
        assert!(
            imported
                .metadata
                .bibliography_content
                .contains("@book{demo")
        );
    }

    /// 分章文件开头写一条分隔线是合法 Markdown，不能当成第二份 frontmatter
    /// 把整次导入挡下来。
    #[test]
    fn research_folder_accepts_thematic_breaks_in_later_files() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("01.md"), "## 第一章\n\n正文。").unwrap();
        std::fs::write(
            dir.path().join("02.md"),
            "---\n\n## 第二章\n\n正文。\n\n---\n\n收尾。",
        )
        .unwrap();

        let imported = import_research_folder(dir.path()).expect("分隔线不应被当成 frontmatter");
        assert!(imported.markdown.contains("## 第二章"));
    }

    /// 封面题名要与 mdx 一致地剥掉手工编号：标题经 frontmatter 传给 mdx 后
    /// 那边不会再清洗一次，这里不剥就会把"一、"印到封面上。
    #[test]
    fn research_folder_strips_manual_numbering_from_the_title() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(
            dir.path().join("01.md"),
            "# 一、某某领域发展研究\n\n## 概述",
        )
        .unwrap();

        let imported = import_research_folder(dir.path()).unwrap();
        assert_eq!(imported.title, "某某领域发展研究");
    }

    /// 缺一张图不该让整次导入失败：正文照常拿到，缺的引用汇报给界面。
    #[test]
    fn research_folder_skips_missing_images_instead_of_failing() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(
            dir.path().join("01.md"),
            "## 概述\n\n![缺的图](figures/missing.png)\n\n正文。",
        )
        .unwrap();

        let imported = import_research_folder(dir.path()).expect("缺图不应阻断导入");
        assert!(imported.markdown.contains("正文。"));
        assert_eq!(imported.skipped_images.len(), 1);
        assert!(imported.skipped_images[0].contains("missing.png"));
    }

    #[test]
    fn research_folder_rejects_later_frontmatter() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("01.md"), "## 第一章").unwrap();
        std::fs::write(
            dir.path().join("02.md"),
            "---\n文件名称: 第二份\n---\n## 第二章",
        )
        .unwrap();
        let error = import_research_folder(dir.path()).unwrap_err();
        assert!(format!("{error:#}").contains("第二份 frontmatter"));
    }
}
