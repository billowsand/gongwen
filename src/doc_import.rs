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
//!
//! 知识库导入（[`to_knowledge_markdown`]）是例外：检索只要文字对、不要版式，
//! 所以收**电子版** PDF；扫描件没有文字层，anydoc 会逐页查出来，这里整份拒收。

use crate::models::ResearchMetadata;
use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

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
    file_dialog(false).pick_file()
}

/// 打开知识库导入的文件选择框（可多选），比起草页多一组电子版 PDF。
pub(crate) fn pick_knowledge_files() -> Option<Vec<PathBuf>> {
    file_dialog(true).pick_files()
}

/// 按 [`FILTER_GROUPS`] 拼文件对话框；`with_pdf` 时追加 PDF 一组。
fn file_dialog(with_pdf: bool) -> rfd::FileDialog {
    let mut all: Vec<&str> = FILTER_GROUPS
        .iter()
        .flat_map(|(_, extensions)| extensions.iter().copied())
        .collect();
    if with_pdf {
        all.push("pdf");
    }
    let mut dialog = rfd::FileDialog::new().add_filter("所有支持的格式", &all);
    for (label, extensions) in FILTER_GROUPS {
        dialog = dialog.add_filter(*label, extensions);
    }
    if with_pdf {
        dialog = dialog.add_filter("PDF（电子版）", &["pdf"]);
    }
    dialog
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
    if extension_of(path) == "pdf" {
        bail!(
            "PDF 不支持导入：PDF 只存版面不存结构，抽出来的正文会丢段落、表格会散架，\
             扫描件更是取不到文字。请改用原始的 Word 文件，或先另存为 docx。"
        );
    }
    convert(path)
}

/// 知识库导入用的转换：比 [`to_markdown`] 多收电子版 PDF，并给没用标题样式的
/// 公文补出章节标题（见 [`promote_headings`]）。Markdown 原文作者自己排过结构，不动。
pub(crate) fn to_knowledge_markdown(path: &Path) -> Result<String> {
    let extension = extension_of(path);
    if extension == "pdf" {
        return Ok(promote_headings(&convert_pdf(path)?));
    }
    let markdown = convert(path)?;
    if matches!(extension.as_str(), "md" | "markdown") {
        Ok(markdown)
    } else {
        Ok(promote_headings(&markdown))
    }
}

/// 小写扩展名，没有扩展名时为空串。
fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 非 PDF 格式的转换：纯文本直读，其余走 `anydoc`。
fn convert(path: &Path) -> Result<String> {
    let extension = extension_of(path);
    let markdown = if PLAIN.contains(&extension.as_str()) {
        crate::text_file::read_to_string(path)
            .with_context(|| format!("读取 {} 失败", file_label(path)))?
    } else if CONVERTED.contains(&extension.as_str()) {
        anydoc::to_markdown(path)
            .map_err(describe_convert_error)
            .with_context(|| format!("解析 {} 失败", file_label(path)))?
    } else {
        bail!(
            "不支持的文件格式 .{extension}：可导入 {}。",
            supported_summary()
        );
    };
    finish(path, &markdown)
}

/// 电子版 PDF 的转换。扫描页（哪怕只有一页）整份拒收：缺了那几页的内容
/// 进了知识库，检索时会被当成"原文里没有"，比不收更误导。
fn convert_pdf(path: &Path) -> Result<String> {
    let markdown = anydoc::to_markdown(path)
        .map_err(|error| match error {
            anydoc::ConvertError::NeedsOcr { pages, page_count } => {
                anyhow!(needs_ocr_message(&pages, page_count))
            }
            other => describe_convert_error(other),
        })
        .with_context(|| format!("解析 {} 失败", file_label(path)))?;
    if looks_garbled(&markdown) {
        bail!(
            "{} 提取出的文字是乱码：PDF 里的字体没有内嵌文字编码映射，取不到真实文字。\
             请改用原始的 Word 文件。",
            file_label(path)
        );
    }
    finish(path, &markdown)
}

/// 扫描版 PDF 的提示语：全是扫描页与只有个别扫描页分开说。
fn needs_ocr_message(pages: &[u32], page_count: u32) -> String {
    if pages.len() as u32 >= page_count {
        return "这是扫描版 PDF，页面只是图片、没有文字层，无法导入。\
                请改用原始的 Word 文件，或先用 OCR 软件识别成文字版。"
            .to_string();
    }
    let mut listed = pages
        .iter()
        .take(5)
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("、");
    if pages.len() > 5 {
        listed.push_str(&format!(" 等 {} 页", pages.len()));
    }
    format!(
        "第 {listed} 页是扫描图片、没有文字层，整份 PDF 未导入（共 {page_count} 页）。\
         请改用原始的 Word 文件，或先用 OCR 软件识别成文字版。"
    )
}

/// 把 anydoc 的常见错误翻成中文；其余原样透出，便于排查。
fn describe_convert_error(error: anydoc::ConvertError) -> anyhow::Error {
    match error {
        anydoc::ConvertError::Encrypted => anyhow!("文件设了密码，请先去掉密码再导入"),
        other => anyhow!("{other}"),
    }
}

/// 转换结果的公共收尾：归一化，并拒收提取不出文字的文件。
fn finish(path: &Path, markdown: &str) -> Result<String> {
    let markdown = normalize(markdown);
    if markdown.is_empty() {
        bail!(
            "{} 里没有提取到文字：文件可能是空的，或者内容全是图片。",
            file_label(path)
        );
    }
    Ok(markdown)
}

/// 判断 PDF 抽出的文字是不是乱码。
///
/// 字体没内嵌 ToUnicode 映射时，anydoc 只记一条日志、照样返回文字，乱码会悄悄进
/// 索引。乱码的样子有两种：替换符、私用区、控制字符这类"根本不是字"的；以及把
/// 字形编号当 Latin-1 解出来的一串带重音的西文字母。前者超过 5%、后者超过 30%
/// 就判为乱码——正常的中英文不会碰到，德法文重音字母也远到不了 30%。
fn looks_garbled(text: &str) -> bool {
    let mut total = 0usize;
    let mut invalid = 0usize;
    let mut latin_extended = 0usize;
    for ch in text.chars().filter(|ch| !ch.is_whitespace()) {
        total += 1;
        if ch == '\u{fffd}' || ('\u{e000}'..='\u{f8ff}').contains(&ch) || ch.is_control() {
            invalid += 1;
        } else if ('\u{80}'..='\u{24f}').contains(&ch) {
            latin_extended += 1;
        }
    }
    total > 0 && (invalid * 20 > total || latin_extended * 10 > total * 3)
}

/// 给没用 Word 标题样式的公文补出章节标题。
///
/// 按国标排的公文，「一、总体要求」这类一级标题多半是直接设黑体三号，没套标题
/// 样式；anydoc 只认标题样式与大纲级别，转出来就是普通段落。知识库切块按 `##`
/// 分节，认不出标题就只能按长度硬切，检索结果里也没有章节路径。这里把"短、不以
/// 句读结尾、以 `一、` 或 `第X章` / `第X部分` 开头"的独立段落升成 `##`，编号原样
/// 保留（预览渲染时 `clean_heading_number` 会先清掉旧编号，不会重复）。
///
/// 已经有 `##` 的文档说明作者用了标题样式，不再猜。`（一）` 这类二级标题不升：
/// 切块器会把 `###` 并回上一节，升了也不改变分块。
fn promote_headings(markdown: &str) -> String {
    if markdown
        .lines()
        .any(|line| line.trim_start().starts_with("## "))
    {
        return markdown.to_string();
    }
    let mut in_code = false;
    markdown
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                in_code = !in_code;
            }
            match heading_candidate(line) {
                Some(title) if !in_code => format!("## {title}"),
                _ => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 判断一行是否像公文一级标题，是则返回去掉整行加粗后的标题文字。
fn heading_candidate(line: &str) -> Option<String> {
    let text = line.trim();
    // 整行加粗是 Word 里手工设粗体的标题。
    let text = text
        .strip_prefix("**")
        .and_then(|inner| inner.strip_suffix("**"))
        .unwrap_or(text)
        .trim();
    // 标题一般不过二三十字；更长的是"一、……。"这种段首带编号的正文段。
    if text.is_empty() || text.chars().count() > 40 {
        return None;
    }
    if text.ends_with(['。', '；', ';', '，', ',', '：', ':', '！', '？', '!', '?']) {
        return None;
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^(?:[一二三四五六七八九十百]+、|第[一二三四五六七八九十百零\d]+(?:章|部分))\s*\S",
        )
        .expect("valid heading pattern")
    });
    re.is_match(text).then(|| text.to_string())
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
    fn knowledge_promotes_bare_level_one_headings() {
        let raw = "关于加强某项工作的通知\n\n一、总体要求\n\n正文一段。\n\n**二、工作安排**\n\n第一章 总则\n\n正文二段。";
        assert_eq!(
            promote_headings(raw),
            "关于加强某项工作的通知\n\n## 一、总体要求\n\n正文一段。\n\n## 二、工作安排\n\n## 第一章 总则\n\n正文二段。"
        );
    }

    #[test]
    fn knowledge_leaves_numbered_paragraphs_alone() {
        // 段首带编号的正文、以句读结尾的短句、二级标题、条文与代码块都不升。
        let long = format!("一、{}", "提高认识，".repeat(10));
        for line in [
            long.as_str(),
            "一、请各单位于5月底前报送。",
            "（一）组织领导",
            "第一条 为规范管理，制定本办法",
            "一、",
        ] {
            assert_eq!(heading_candidate(line), None, "{line}");
        }
        let fenced = "```\n一、示例\n```";
        assert_eq!(promote_headings(fenced), fenced);
    }

    #[test]
    fn knowledge_keeps_existing_heading_styles() {
        let raw = "## 一、总体要求\n\n二、工作安排";
        assert_eq!(promote_headings(raw), raw);
    }

    #[test]
    fn knowledge_markdown_is_not_rewritten() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("底稿.md");
        std::fs::write(&path, "一、总体要求\n\n正文。").expect("写入");
        assert_eq!(
            to_knowledge_markdown(&path).unwrap(),
            "一、总体要求\n\n正文。"
        );
        let text = dir.path().join("底稿.txt");
        std::fs::write(&text, "一、总体要求\n\n正文。").expect("写入");
        assert_eq!(
            to_knowledge_markdown(&text).unwrap(),
            "## 一、总体要求\n\n正文。"
        );
    }

    #[test]
    fn garbled_text_is_detected() {
        assert!(!looks_garbled("关于加强某项工作的通知。Hello, world!"));
        assert!(!looks_garbled("Die Größe der Übung für Äpfel"));
        assert!(looks_garbled("ÄÖÜ×Þßàáâãäåæçèéêëìíîïðñòóôõöøùúûüýþ"));
        assert!(looks_garbled("正文\u{fffd}\u{fffd}\u{e001}\u{e002}"));
    }

    #[test]
    fn needs_ocr_message_distinguishes_whole_and_partial_scans() {
        assert!(needs_ocr_message(&[1, 2], 2).contains("扫描版 PDF"));
        let partial = needs_ocr_message(&[2, 3, 4, 5, 6, 7], 10);
        assert!(partial.contains("第 2、3、4、5、6 等 6 页"), "{partial}");
        assert!(partial.contains("共 10 页"), "{partial}");
    }

    /// 手搓一份单页 PDF：`resources` 是页面资源字典，`content` 是内容流。
    fn minimal_pdf(resources: &str, content: &str, extra_objects: &[String]) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources {resources} /Contents 4 0 R >>"
            ),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        objects.extend(extra_objects.iter().cloned());
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    #[test]
    fn knowledge_accepts_text_pdf() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("电子版.pdf");
        let pdf = minimal_pdf(
            "<< /Font << /F1 5 0 R >> >>",
            "BT /F1 12 Tf 72 760 Td (Annual work report of the office) Tj ET",
            &["<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string()],
        );
        std::fs::write(&path, pdf).expect("写入");
        let markdown = to_knowledge_markdown(&path).unwrap();
        assert!(markdown.contains("Annual work report"), "{markdown}");
        // 起草页仍然不收 PDF。
        assert!(to_markdown(&path).is_err());
    }

    #[test]
    fn knowledge_rejects_scanned_pdf() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("扫描件.pdf");
        // 整页只画一张 8×8 灰度图、没有任何文字，就是扫描件的样子。
        let pixels = "80".repeat(64);
        let pdf = minimal_pdf(
            "<< /XObject << /Im1 5 0 R >> >>",
            "q 595 0 0 842 0 0 cm /Im1 Do Q",
            &[format!(
                "<< /Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray \
                 /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length {} >>\nstream\n{pixels}>\nendstream",
                pixels.len() + 1
            )],
        );
        std::fs::write(&path, pdf).expect("写入");
        let message = format!("{:#}", to_knowledge_markdown(&path).unwrap_err());
        assert!(message.contains("扫描版 PDF"), "{message}");
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

    /// anydoc 自 0.2 起用自带的表格解析器，与本软件词库导入用的 calamine 各走各的；
    /// 两套同时链进来是否相安无事，只有真跑一遍才知道——这里现造一个 xlsx 再转回 markdown。
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

    /// Word 公式（OMML）要转成研究报告认得的 `$...$` / `$$` 块。anydoc 0.1 会把
    /// 公式整个丢掉、正文只剩一个空位且不报错，这条测试防止倒退。
    #[test]
    fn word_equations_become_latex_math() {
        use std::io::Write as _;

        const MATH_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
        let inline = "<m:oMath><m:f><m:num><m:r><m:t>a</m:t></m:r></m:num>\
                      <m:den><m:r><m:t>b</m:t></m:r></m:den></m:f></m:oMath>";
        let display = "<m:oMathPara><m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e>\
                       <m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath></m:oMathPara>";
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:m="{MATH_NS}"><w:body>
<w:p><w:r><w:t xml:space="preserve">比值为 </w:t></w:r>{inline}<w:r><w:t>，平方如下：</w:t></w:r></w:p>
<w:p>{display}</w:p>
</w:body></w:document>"#
        );
        let parts = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
            ),
            ("word/document.xml", document.as_str()),
        ];

        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("含公式.docx");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).expect("创建 docx"));
        for (name, content) in parts {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .expect("写入 docx 部件");
            zip.write_all(content.as_bytes()).expect("写入 docx 部件");
        }
        zip.finish().expect("收尾 docx");

        let markdown = to_markdown(&path).unwrap();
        assert!(
            markdown.contains(r"比值为 $\frac{a}{b}$，平方如下："),
            "{markdown}"
        );
        assert!(markdown.contains("$$\nx^{2}\n$$"), "{markdown}");
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
