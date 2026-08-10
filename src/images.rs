//! 图片资源：插入的图片统一复制到配置目录 `images/` 目录下，文档 markdown 用
//! 相对路径（`images/xxx.png`）引用，预览与导出都按配置目录解析。
//!
//! 设计要点：
//! - 唯一命名 `yyyyMMdd_HHmmss_<净化文件名>`，避免跨稿件、跨会话覆盖；
//! - 相对路径随 ZIP 备份/恢复（见 manuscript_io），原文件删除不影响文档；
//! - 图片文件长期保留：版本快照（manuscript_versions）只存 markdown 文本，
//!   回滚版本不影响图片文件，旧版本里的图片引用依然有效；
//! - 删除稿件时清理仅被该稿件引用的孤儿图片（`manuscript::purge_orphan_images`），
//!   仍被其他稿件引用或库外引用的图片一律保留，避免误删；
//! - PDF 作为文件入库，预览只显示占位，导出由各导出器自行嵌入；
//! - 图片必须独占一行（`![alt](src)` 一行），插入时自动在前后补空行保证；
//! - 拒绝绝对路径与 `..` 穿越，防止 markdown 引用读取任意文件。

use crate::storage;
use anyhow::{Context, Result, bail};
use chrono::Local;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// 相对路径目录名（相对配置目录）。
pub(crate) const IMAGE_DIR: &str = "images";

/// 支持的图片/附件扩展名（小写、不带点）。
/// svg 不在列：docx-rs 的 Pic 无法把 svg 转 PNG，导出会失败。
pub(crate) const SUPPORTED_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "webp", "bmp", "gif", "pdf"];

/// 与解析器（export::parse_markdown_located 的独占行识别）保持一致的图片引用正则。
/// src 不含空格、换行与右括号——入库文件名经过净化，天然满足。
static IMAGE_REF_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"!\[[^\]\n]*\]\(\s*([^)\s]+)\s*\)").expect("图片引用正则必须合法")
});

/// 一次导入产出的图片记录。
#[derive(Debug, Clone)]
pub(crate) struct ImportedImage {
    /// markdown 引用用的相对路径，如 `images/20260809_121500_扫描件.png`。
    pub rel_path: String,
    /// 可直接插入文档的 markdown 片段，如 `![扫描件](images/20260809_121500_扫描件.png)`。
    pub markdown: String,
}

/// 图片资源目录：配置目录/images/。
pub(crate) fn image_dir() -> Result<PathBuf> {
    Ok(storage::config_dir()?.join(IMAGE_DIR))
}

/// 把 markdown 中的相对路径解析为绝对路径。
/// 拒绝绝对路径与 `..` 穿越，防止恶意 markdown 读取任意文件。
pub(crate) fn resolve(rel: &str) -> Result<PathBuf> {
    resolve_from(&storage::config_dir()?, rel)
}

/// 以给定目录为基准解析相对路径（拒绝绝对路径与 `..` 穿越），供导出/备份复用。
pub(crate) fn resolve_from_base(base: &Path, rel: &str) -> Result<PathBuf> {
    resolve_from(base, rel)
}

fn resolve_from(base: &Path, rel: &str) -> Result<PathBuf> {
    let path = Path::new(rel);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("非法图片路径：{rel}");
    }
    Ok(base.join(rel))
}

/// 打开"插入图片"的文件选择框（可多选），返回用户选中的文件；取消返回 None。
pub(crate) fn pick_files() -> Option<Vec<PathBuf>> {
    let bitmaps = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];
    let mut dialog = rfd::FileDialog::new().add_filter("图片 / PDF", SUPPORTED_EXTENSIONS);
    dialog = dialog.add_filter("PNG / JPG / WebP / BMP / GIF", &bitmaps);
    dialog = dialog.add_filter("PDF 附件", &["pdf"]);
    dialog.pick_files()
}

/// 把选中文件复制入库，返回每个文件的相对路径与 markdown 片段。
/// 不支持扩展名的文件被跳过；空输入返回空列表。
pub(crate) fn import(files: &[PathBuf]) -> Result<Vec<ImportedImage>> {
    import_into(&image_dir()?, files)
}

fn import_into(dir: &Path, files: &[PathBuf]) -> Result<Vec<ImportedImage>> {
    fs::create_dir_all(dir).with_context(|| format!("无法创建图片目录 {}", dir.display()))?;
    let stamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let mut out = Vec::new();
    for file in files {
        let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_supported(file) {
            continue;
        }
        let bytes =
            fs::read(file).with_context(|| format!("无法读取图片文件 {}", file.display()))?;
        let stored_name = format!("{stamp}_{}", sanitize_file_name(name));
        let rel_path = format!("{IMAGE_DIR}/{stored_name}");
        let dest = dir.join(&stored_name);
        // 同秒重复导入同一文件：内容相同，直接复用避免覆盖窗口。
        if !dest.exists() {
            fs::write(&dest, &bytes)
                .with_context(|| format!("无法写入图片文件 {}", dest.display()))?;
        }
        let alt = sanitize_alt(
            Path::new(name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        let markdown = format!("![{alt}]({rel_path})");
        out.push(ImportedImage { rel_path, markdown });
    }
    Ok(out)
}

/// 净化文件名：剔除路径分隔符、控制字符、Windows 非法字符与 markdown 特殊字符，
/// 保留扩展名。括号会截断 `![alt](src)` 的 src 解析，必须一并剔除。
fn sanitize_file_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_whitespace()
                || c == '/'
                || c == '\\'
                || c.is_control()
                || "<>:\"|?*()[]".contains(c)
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    if out.trim().is_empty() {
        out = "image".into();
    }
    out
}

/// 净化 alt 文本：剔除会提前结束 `![alt](...)` 语法的字符。
fn sanitize_alt(name: String) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c == '[' || c == ']' || c == '(' || c == ')' || c == '`' {
                '_'
            } else {
                c
            }
        })
        .collect();
    if out.trim().is_empty() {
        out = "图片".into();
    }
    out
}

/// 是否支持该扩展名。
pub(crate) fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.as_str()))
}

/// 规范化 markdown 图片引用：统一分隔符为 `/`、去掉 `./` 前缀，让手写引用
/// （`images\a.png`、`./images/a.png`）与入库引用（`images/a.png`）可以互相
/// 比较，避免孤儿清理时把同义的引用误判为不同而误删文件。
pub(crate) fn normalize_ref(src: &str) -> String {
    let src = src.replace('\\', "/");
    let src = src.strip_prefix("./").unwrap_or(&src);
    src.to_string()
}

/// 扫描 markdown 中的图片引用 `![alt](src)`，返回去重保序的 src 列表。
/// 供导出、备份复制图片文件用。
pub(crate) fn image_refs(markdown: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for caps in IMAGE_REF_RE.captures_iter(markdown) {
        if let Some(src) = caps.get(1).map(|m| m.as_str().trim().to_string())
            && !src.is_empty()
            && seen.insert(src.clone())
        {
            out.push(src);
        }
    }
    out
}

/// 把 markdown 引用的图片文件复制到目标目录（保持 `images/` 相对结构），
/// 让导出的 md/tex 目录自包含。引用缺失或读取失败时跳过，不阻断导出。
pub(crate) fn copy_refs(markdown: &str, target_dir: &Path) -> Result<()> {
    copy_refs_from(&storage::config_dir()?, markdown, target_dir)
}

fn copy_refs_from(base: &Path, markdown: &str, target_dir: &Path) -> Result<()> {
    for src in image_refs(markdown) {
        let source = match resolve_from(base, &src) {
            Ok(source) => source,
            Err(_) => continue,
        };
        if !source.is_file() {
            continue;
        }
        let dest = target_dir.join(&src);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建图片输出目录 {}", parent.display()))?;
        }
        fs::copy(&source, &dest)
            .with_context(|| format!("无法复制图片 {} 到 {}", source.display(), dest.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write as _;

    fn write_tmp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut f = File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn sanitize_file_names() {
        assert_eq!(sanitize_file_name("扫描件.png"), "扫描件.png");
        assert_eq!(sanitize_file_name("a/b\\c:d*e?.png"), "a_b_c_d_e_.png");
        assert_eq!(sanitize_file_name(""), "image");
        // 括号会截断 ![alt](src) 的 src 解析，必须净化掉。
        assert_eq!(sanitize_file_name("图(1).png"), "图_1_.png");
        assert_eq!(sanitize_file_name("附件[扫描].pdf"), "附件_扫描_.pdf");
        assert_eq!(sanitize_alt("图[1](x)".into()), "图_1__x_");
        assert_eq!(sanitize_alt("".into()), "图片");
    }

    #[test]
    fn supported_extensions() {
        assert!(is_supported(Path::new("x.PNG")));
        assert!(is_supported(Path::new("x.jpeg")));
        assert!(is_supported(Path::new("x.pdf")));
        assert!(!is_supported(Path::new("x.svg")));
        assert!(!is_supported(Path::new("x.txt")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn import_copies_files_and_builds_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let png = write_tmp(src.path(), "扫描 件.png", b"fake-png");
        let pdf = write_tmp(src.path(), "附件.pdf", b"%PDF-1.4");
        let txt = write_tmp(src.path(), "说明.txt", b"skip me");
        let dir = tmp.path().join(IMAGE_DIR); // 模拟 config_dir()/images
        let imgs = import_into(&dir, &[png.clone(), pdf, txt]).unwrap();
        assert_eq!(imgs.len(), 2);
        for img in &imgs {
            assert!(img.rel_path.starts_with("images/"));
            assert!(img.rel_path.ends_with("_扫描_件.png") || img.rel_path.ends_with("_附件.pdf"));
            assert!(img.markdown.starts_with("!["));
            // 文件确实写入了，且相对路径可经 resolve 定位。
            let stored = resolve_from(tmp.path(), &img.rel_path).unwrap();
            assert!(stored.exists());
        }
        assert!(imgs[0].markdown.contains("扫描_件"));
    }

    #[test]
    fn import_reuses_existing_file_on_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let png = write_tmp(src.path(), "a.png", b"png");
        let first = import_into(tmp.path(), std::slice::from_ref(&png)).unwrap();
        let second = import_into(tmp.path(), std::slice::from_ref(&png)).unwrap();
        assert_eq!(first[0].rel_path, second[0].rel_path);
    }

    #[test]
    fn resolve_rejects_escape_and_accepts_relative() {
        let base = Path::new("/base");
        assert_eq!(
            resolve_from(base, "images/a.png").unwrap(),
            PathBuf::from("/base/images/a.png")
        );
        assert!(resolve_from(base, "../a.png").is_err());
        assert!(resolve_from(base, "images/../../a.png").is_err());
        assert!(resolve_from(base, "/etc/passwd").is_err());
        // 带盘符的绝对路径只在 Windows 上成立，Unix 上 `C:\evil.png` 只是普通相对名。
        #[cfg(windows)]
        assert!(resolve_from(base, "C:\\evil.png").is_err());
    }

    #[test]
    fn image_refs_scans_and_deduplicates() {
        let md = "![a](images/1.png)\n正文\n![b](images/2.pdf)\n![重复](images/1.png)";
        assert_eq!(image_refs(md), vec!["images/1.png", "images/2.pdf"]);
        assert!(image_refs("没有图片").is_empty());
        // 带空格 src 的引用（手写）不匹配，与解析器约束一致。
        assert!(image_refs("![a](images/my file.png)").is_empty());
    }

    #[test]
    fn normalize_ref_unifies_separators_and_prefix() {
        assert_eq!(normalize_ref("images/a.png"), "images/a.png");
        assert_eq!(normalize_ref("images\\a.png"), "images/a.png");
        assert_eq!(normalize_ref("./images/a.png"), "images/a.png");
        assert_eq!(normalize_ref(".\\images\\a.png"), "images/a.png");
        assert_eq!(normalize_ref("a.png"), "a.png");
    }

    #[test]
    fn copy_refs_copies_mentioned_images_preserving_relative_layout() {
        let base = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let img_dir = base.path().join("images");
        fs::create_dir_all(&img_dir).unwrap();
        fs::write(img_dir.join("a.png"), b"png-a").unwrap();
        fs::write(img_dir.join("b.pdf"), b"%PDF").unwrap();
        // a、b 被引用会复制；c 未被引用不复制；缺文件跳过不报错。
        let md = "![a](images/a.png)\n![b](images/b.pdf)\n![缺失](images/missing.png)";
        copy_refs_from(base.path(), md, target.path()).unwrap();
        assert_eq!(
            fs::read(target.path().join("images/a.png")).unwrap(),
            b"png-a"
        );
        assert_eq!(
            fs::read(target.path().join("images/b.pdf")).unwrap(),
            b"%PDF"
        );
        assert!(!target.path().join("images/missing.png").exists());
        // 未引用的图片不复制。
        assert!(!target.path().join("images/c.png").exists());
        // 非法路径（穿越）被跳过。
        copy_refs_from(base.path(), "![x](../etc/passwd)", target.path()).unwrap();
    }
}
