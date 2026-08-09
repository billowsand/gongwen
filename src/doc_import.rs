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

use anyhow::{Context, Result, bail};
use std::path::Path;

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
        std::fs::read_to_string(path).with_context(|| {
            format!(
                "读取 {} 失败（若文件是 GBK 编码，请先另存为 UTF-8）",
                file_label(path)
            )
        })?
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
}
