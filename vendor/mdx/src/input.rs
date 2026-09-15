use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use walkdir::WalkDir;

/// 校验输入路径并区分单文件/目录
pub fn classify(input: &Path) -> Result<InputKind> {
    if !input.exists() {
        anyhow::bail!("找不到输入路径 '{}'", input.display());
    }
    if input.is_dir() {
        return Ok(InputKind::Directory);
    }
    if input.is_file()
        && input
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md"))
            .unwrap_or(false)
    {
        return Ok(InputKind::File);
    }
    anyhow::bail!("'{}' 必须是目录或 .md 文件", input.display());
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputKind {
    File,
    Directory,
}

/// 把输入路径合并成单一字符串（目录则按文件名升序拼接所有 .md）
pub fn collect_raw(input: &Path) -> Result<String> {
    match classify(input)? {
        InputKind::File => read_markdown(input),
        InputKind::Directory => merge_dir(input),
    }
}

/// 读取单个 Markdown 文件，并移除 UTF-8 BOM。
///
/// 目录输入会把多个文件串接起来；若只在合并后的整段文本开头去 BOM，第二个及
/// 后续文件的 BOM 会残留在首行，导致 `## 标题` 实际变成 `\u{feff}## 标题`，
/// 从而被解析器误判为普通段落。因此必须在合并前逐文件处理。
fn read_markdown(path: &Path) -> Result<String> {
    let content =
        fs::read_to_string(path).with_context(|| format!("读取文件 {} 失败", path.display()))?;
    Ok(content
        .strip_prefix('\u{feff}')
        .unwrap_or(&content)
        .to_owned())
}

/// 删除整行只剩 `---` / `----` …… 的水平分隔线。
///
/// 表格分隔行（如 `|---|---|`）以 `|` 起始，正则不会命中；不影响表格解析。
pub(crate) fn strip_horizontal_rules(content: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\s*-{3,}\s*$").expect("invalid hr regex"));
    content
        .lines()
        .filter(|line| !re.is_match(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn merge_dir(dir: &Path) -> Result<String> {
    let mut md_files: Vec<PathBuf> = WalkDir::new(dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    if md_files.is_empty() {
        anyhow::bail!("在目录 '{}' 中未找到任何 .md 文件", dir.display());
    }

    md_files.sort_by(|a, b| {
        let na = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let nb = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        na.cmp(nb)
    });

    println!("找到 {} 个 markdown 文件:", md_files.len());
    for f in &md_files {
        println!(
            "  - {}",
            f.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    let mut merged = String::new();
    for path in &md_files {
        let content = read_markdown(path)?;
        merged.push_str(&content);
        merged.push_str("\n\n");
    }
    Ok(merged)
}

/// 计算默认输出路径（相对于落地目录）。
///
/// tex 会连带产出 `data/`、`appendix/`、`figures/`、`.cls`、`.bib` 以及 PDF，
/// 散在落地目录里很乱，因此统一收进一个单独的目录：
/// - 单文件 `report.md` → `report/report.tex`
/// - 目录 `chapters/`   → `chapters-tex/chapters.tex`（加后缀避免与输入目录同名冲突）
///
/// docx 是自包含单文件，仍直接落在落地目录下（`report.docx`）。
pub fn default_output(input: &Path, ext: &str) -> PathBuf {
    let kind = classify(input).unwrap_or(InputKind::File);
    let name = base_name(input, kind);
    let file = format!("{}.{}", name, ext);
    match bundle_dir_name(&name, kind, ext) {
        Some(dir) => PathBuf::from(dir).join(file),
        None => PathBuf::from(file),
    }
}

/// 需要单独收纳配套文件的格式，返回其目录名；自包含格式返回 None。
fn bundle_dir_name(name: &str, kind: InputKind, ext: &str) -> Option<String> {
    if !ext.eq_ignore_ascii_case("tex") {
        return None;
    }
    Some(match kind {
        // 输出目录与输入目录同名会互相覆盖，加 -tex 后缀区分
        InputKind::Directory => format!("{}-tex", name),
        InputKind::File => name.to_owned(),
    })
}

/// 输出使用的基名：目录取目录名，单文件取 stem。
/// `.` / `..` 这类相对路径没有可用的 file_name，规范化后再取真实目录名。
fn base_name(input: &Path, kind: InputKind) -> String {
    let raw = match kind {
        InputKind::Directory => input.file_name(),
        InputKind::File => input.file_stem(),
    };
    raw.and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            fs::canonicalize(input).ok().and_then(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_owned)
                    .filter(|n| !n.is_empty())
            })
        })
        .unwrap_or_else(|| "output".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_raw_preserves_opening_front_matter() {
        let dir = tempfile::tempdir().unwrap();
        let md = dir.path().join("paper.md");
        fs::write(&md, "---\nbibliography: refs.bib\n---\n正文\n").unwrap();

        let raw = collect_raw(&md).unwrap();

        assert!(raw.starts_with("---\nbibliography: refs.bib\n---"));
    }

    #[test]
    fn collect_raw_strips_bom_from_every_directory_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("01.md"), "\u{feff}## 第一章 标题\n").unwrap();
        fs::write(dir.path().join("02.md"), "\u{feff}## 第二章 标题\n").unwrap();

        let raw = collect_raw(dir.path()).unwrap();
        let headings: Vec<(u8, String)> = crate::parser::parse(&raw)
            .into_iter()
            .filter_map(|block| match block {
                crate::common::ast::Block::Heading { level, text } => Some((level, text)),
                _ => None,
            })
            .collect();

        assert!(!raw.contains('\u{feff}'));
        assert_eq!(
            headings,
            vec![(2, "标题".to_string()), (2, "标题".to_string())]
        );
    }

    #[test]
    fn tex_output_goes_into_its_own_directory() {
        let dir = tempfile::tempdir().unwrap();
        let md = dir.path().join("report.md");
        fs::write(&md, "正文\n").unwrap();

        assert_eq!(
            default_output(&md, "tex"),
            PathBuf::from("report").join("report.tex")
        );
    }

    #[test]
    fn tex_output_dir_for_input_directory_gets_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let chapters = dir.path().join("chapters");
        fs::create_dir(&chapters).unwrap();
        fs::write(chapters.join("01.md"), "正文\n").unwrap();

        assert_eq!(
            default_output(&chapters, "tex"),
            PathBuf::from("chapters-tex").join("chapters.tex")
        );
    }

    #[test]
    fn docx_output_stays_a_bare_file() {
        let dir = tempfile::tempdir().unwrap();
        let md = dir.path().join("report.md");
        fs::write(&md, "正文\n").unwrap();
        let chapters = dir.path().join("chapters");
        fs::create_dir(&chapters).unwrap();
        fs::write(chapters.join("01.md"), "正文\n").unwrap();

        assert_eq!(default_output(&md, "docx"), PathBuf::from("report.docx"));
        assert_eq!(
            default_output(&chapters, "docx"),
            PathBuf::from("chapters.docx")
        );
    }

    #[test]
    fn dot_directory_falls_back_to_canonical_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("01.md"), "正文\n").unwrap();
        let real_name = fs::canonicalize(dir.path())
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        assert_eq!(
            default_output(&dir.path().join("."), "tex"),
            PathBuf::from(format!("{real_name}-tex")).join(format!("{real_name}.tex"))
        );
    }
}
