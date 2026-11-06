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
//! - PDF 作为文件入库：预览取第一页光栅化后画在版心里（`preview::pdf_figure`），
//!   导出仍按原矢量由各导出器嵌入；
//! - 图片必须独占一行（`![alt](src)` 一行），插入时自动在前后补空行保证；
//! - 拒绝绝对路径与 `..` 穿越，防止 markdown 引用读取任意文件。

use crate::app::file_dialog::FileDialogSpec;
use crate::storage;
use anyhow::{Context, Result, bail};
use chrono::Local;
use regex::Regex;
use std::collections::{HashMap, HashSet};
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

/// 图片资源目录：配置目录/images/（**槽位感知**：诱饵系统用 `alt-images/`）。
///
/// 诱饵系统必须与真实系统物理隔离：同一台机器上的两份数据不能互相看见。
/// 目录名映射走 [`storage::slot_subdir`]，与 `wipe_slot_files` 的擦除列表一致。
pub(crate) fn image_dir() -> Result<PathBuf> {
    image_dir_for(storage::current_slot())
}

/// 指定槽位的图片目录；不读进程级槽位，供测试与擦除逻辑使用。
pub(crate) fn image_dir_for(slot: storage::DataSlot) -> Result<PathBuf> {
    storage::slot_subdir_for(slot, IMAGE_DIR)
}

/// 把 markdown 中的相对路径解析为绝对路径。
/// 拒绝绝对路径与 `..` 穿越，防止恶意 markdown 读取任意文件。
pub(crate) fn resolve(rel: &str) -> Result<PathBuf> {
    resolve_from_base(&storage::config_dir()?, rel)
}

/// 以给定目录为基准解析相对路径（拒绝绝对路径与 `..` 穿越），供导出/备份复用。
///
/// 配置目录是**槽位命名空间**的唯一基准：`images/` 前缀要映射到当前槽位的图片
/// 目录（真实 `images/`、诱饵 `alt-images/`），正文里的引用仍统一写
/// `images/xxx.png`，跨槽/跨机器都能读。样张与测试传进来的临时基准目录保持
/// 原样 join，不参与槽位映射。
pub(crate) fn resolve_from_base(base: &Path, rel: &str) -> Result<PathBuf> {
    let path = Path::new(rel);
    validate_relative(path, rel)?;
    if base == storage::config_dir()? {
        return app_relative(path);
    }
    Ok(base.join(path))
}

/// 配置目录内的相对引用 → 绝对路径；`images/` 前缀按当前槽位落到 `image_dir()`。
fn app_relative(path: &Path) -> Result<PathBuf> {
    let Some(first) = path.components().next() else {
        anyhow::bail!("非法图片路径：空");
    };
    // 正文只能使用逻辑目录名，由这里映射到当前槽位。显式写 alt-* 会绕过
    // 映射直接读到另一槽；配置目录命名空间里一律拒绝，外部基准目录不受影响。
    if first
        .as_os_str()
        .to_string_lossy()
        .starts_with(storage::DECOY_PREFIX)
    {
        anyhow::bail!("非法图片路径：不允许显式引用另一数据槽位");
    }
    if first.as_os_str() == storage::MERMAID_CACHE_DIR {
        // Mermaid 缓存同样是用户正文派生物：按槽位落目录，缺图失败的判定交给
        // 读取方（加密文件解不开时缓存会重新渲染）。
        let rest = path
            .strip_prefix(storage::MERMAID_CACHE_DIR)
            .unwrap_or(Path::new(""));
        return Ok(storage::slot_subdir(storage::MERMAID_CACHE_DIR)?.join(rest));
    }
    if first.as_os_str() != IMAGE_DIR {
        return Ok(storage::config_dir()?.join(path));
    }
    let rest = path.strip_prefix(IMAGE_DIR).unwrap_or(Path::new(""));
    let slot_path = image_dir()?.join(rest);
    // 使用帮助的截图是随包公开资源，不随槽位复制。诱饵槽里没有副本时回落到
    // 真实槽的共享目录；用户图片不做这个回落，避免跨槽泄露内容。
    if let Some(name) = rest.file_name().and_then(|name| name.to_str())
        && !slot_path.is_file()
        && let Ok(shared) = storage::config_dir()
    {
        let shared = shared.join(IMAGE_DIR).join(rest);
        // 名称相同不能证明内容公开：用户或旧版程序可能覆盖过这张图。
        // 只有与随包公开字节完全一致的副本才可跨槽使用。
        if public_help_copy(&shared, name) {
            return Ok(shared);
        }
    }
    Ok(slot_path)
}

fn public_help_copy(path: &Path, name: &str) -> bool {
    crate::help::content::IMAGES
        .iter()
        .find(|(public, _)| *public == name)
        .is_some_and(|(_, bytes)| {
            crate::file_limits::read_bytes(path, bytes.len() as u64, "帮助截图")
                .is_ok_and(|actual| actual == *bytes)
        })
}

/// 读取应用管理的资源文件（图片、PDF 插图、Mermaid 缓存与警告）。
///
/// 启用应用锁后这些文件是整份 AEAD 密文（与 config.json 同一个 `GWA1` 封套）：
/// 有句柄就解密；**没有句柄时按读不到处理**，绝不把密文当图片渲染。外部导入来源
/// （用户自己选的文件夹）不经过这里——那是不受应用管理的原件。
pub(crate) fn read_managed_file(path: &Path) -> Result<Vec<u8>> {
    read_managed_file_limited(path, crate::file_limits::MAX_IMAGE_BYTES, "资源文件")
}

/// 带体积上限的 [`read_managed_file`]：`max_bytes` 始终限制解密后的明文；
/// 原始读取额外允许一个固定 GWA1 封套，未密封文件仍按原上限拒绝。
pub(crate) fn read_managed_file_limited(
    path: &Path,
    max_bytes: u64,
    what: &str,
) -> Result<Vec<u8>> {
    let raw_limit = max_bytes.saturating_add(crate::vault::SEALED_FILE_OVERHEAD as u64);
    let raw = crate::file_limits::read_bytes(path, raw_limit, what)
        .with_context(|| format!("无法读取{} {}", what, path.display()))?;
    if !crate::vault::is_sealed_file(&raw) {
        if raw.len() as u64 > max_bytes {
            return Err(crate::file_limits::too_large(what, max_bytes).into());
        }
        return Ok(raw);
    }
    let handle = crate::vault::key_handle()
        .with_context(|| format!("{} 已加密，尚未解锁：{}", what, path.display()))?;
    let plain = crate::vault::open_path_bytes(
        &handle,
        path,
        crate::vault::resource_kind_for_path(path)?,
        &raw,
    )
    .with_context(|| {
        format!(
            "{} 解密失败（密钥不匹配或文件损坏）：{}",
            what,
            path.display()
        )
    })?;
    if plain.len() as u64 > max_bytes {
        return Err(crate::file_limits::too_large(what, max_bytes).into());
    }
    Ok(plain)
}

/// 写入应用管理的资源文件：有句柄时密封，原子写 + Unix 0600。没有句柄时写明文
/// （未启用应用锁的系统）；若目标已是密文则拒绝写明文，避免静默降级。
pub(crate) fn write_managed_file(path: &Path, bytes: &[u8]) -> Result<()> {
    // 普通模式的样张/临时输出可以位于配置目录外，此时用途不参与封套；
    // 一旦持有密钥，仍由 GWA2 的配置根与槽位校验拒绝越界写入。
    let kind = if crate::vault::key_handle().is_some() {
        crate::vault::resource_kind_for_path(path)?
    } else {
        "image"
    };
    crate::vault::write_path(path, kind, bytes)
}

/// 随包公开的帮助截图（名称与字节都与内置资源一致）：属于公开内容，不做槽位复制，
/// 也不参与图片加密迁移；诱饵槽可安全回落到真实槽的共享副本。
pub(crate) fn is_public_help_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| public_help_copy(path, name))
}

/// 把一份资源按当前槽位/密钥写入图片目录；供迁移与同步导入共用。
pub(crate) fn write_image_file(path: &Path, bytes: &[u8]) -> Result<()> {
    write_managed_file(path, bytes)
}

/// 外部基准目录的解析：只做路径穿越校验后 join，不参与槽位映射。
/// 导入 Markdown 时 base 是用户选的外部文件夹，不能把它当成配置目录。
fn resolve_from(base: &Path, rel: &str) -> Result<PathBuf> {
    let path = Path::new(rel);
    validate_relative(path, rel)?;
    Ok(base.join(path))
}

fn validate_relative(path: &Path, rel: &str) -> Result<()> {
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
    Ok(())
}

/// "插入图片"的文件选择框参数（可多选）。对话框由应用层在后台线程开。
pub(crate) fn pick_files_spec() -> FileDialogSpec {
    let bitmaps = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];
    FileDialogSpec::pick_files()
        .filter("图片 / PDF", SUPPORTED_EXTENSIONS)
        .filter("PNG / JPG / WebP / BMP / GIF", &bitmaps)
        .filter("PDF 附件", &["pdf"])
}

/// 把选中文件复制入库，返回每个文件的相对路径与 markdown 片段。
/// 不支持扩展名的文件被跳过；空输入返回空列表。
pub(crate) fn import(files: &[PathBuf]) -> Result<Vec<ImportedImage>> {
    import_into(&image_dir()?, files)
}

fn import_into(dir: &Path, files: &[PathBuf]) -> Result<Vec<ImportedImage>> {
    fs::create_dir_all(dir).with_context(|| format!("无法创建图片目录 {}", dir.display()))?;
    let stamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let mut created = Vec::new();
    let result = (|| {
        let mut out = Vec::new();
        for file in files {
            let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !is_supported(file) {
                continue;
            }
            let bytes = crate::file_limits::read_bytes(
                file,
                crate::file_limits::MAX_IMAGE_BYTES,
                "图片文件",
            )
            .with_context(|| format!("无法读取图片文件 {}", file.display()))?;
            let rel_path = store(dir, &stamp, name, &bytes, &mut created)?;
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
    })();
    if result.is_err() {
        for path in created {
            let _ = fs::remove_file(path);
        }
    }
    result
}

/// 把内存里的图片字节入库，返回 markdown 引用用的相对路径，顺序与输入一致。
/// 供文档导入用：Word 等文档里内嵌的图片由转换器取出，没有现成的文件可复制。
/// `name` 是带扩展名的建议文件名，同样会被净化、加时间戳；任一张写失败时本批
/// 已写入的文件全部删掉。
pub(crate) fn import_bytes(items: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    import_bytes_into(&image_dir()?, items)
}

pub(crate) fn import_bytes_into(dir: &Path, items: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(dir).with_context(|| format!("无法创建图片目录 {}", dir.display()))?;
    let stamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let mut created = Vec::new();
    let result = items
        .iter()
        .map(|(name, bytes)| store(dir, &stamp, name, bytes, &mut created))
        .collect::<Result<Vec<_>>>();
    if result.is_err() {
        for path in created {
            let _ = fs::remove_file(path);
        }
    }
    result
}

/// 把一份图片字节写进资源目录，返回相对路径。同名文件内容相同就直接复用，内容
/// 不同则在文件名后加 `_2`、`_3`……；新写入的文件记进 `created`，供失败时回滚。
fn store(
    dir: &Path,
    stamp: &str,
    name: &str,
    bytes: &[u8],
    created: &mut Vec<PathBuf>,
) -> Result<String> {
    let sanitized = sanitize_file_name(name);
    let mut stored_name = format!("{stamp}_{sanitized}");
    let mut dest = dir.join(&stored_name);
    let mut suffix = 2usize;
    while dest.exists() && read_managed_file(&dest).ok().as_deref() != Some(bytes) {
        let source = Path::new(&sanitized);
        let stem = source
            .file_stem()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let extension = source
            .extension()
            .map(|value| format!(".{}", value.to_string_lossy()))
            .unwrap_or_default();
        stored_name = format!("{stamp}_{stem}_{suffix}{extension}");
        dest = dir.join(&stored_name);
        suffix += 1;
    }
    if !dest.exists() {
        write_managed_file(&dest, bytes)
            .with_context(|| format!("无法写入图片文件 {}", dest.display()))?;
        created.push(dest);
    }
    Ok(format!("{IMAGE_DIR}/{stored_name}"))
}

/// 把外部 Markdown 引用的本地图片复制入应用资源库并改写引用，返回改写后的
/// 正文和跳过的引用清单。
///
/// 路径都相对给定基准目录解析；绝对路径与父目录穿越一律拒绝（恶意 Markdown 不
/// 能借此读到资源库外的文件），而**缺失的文件、不支持的格式和网络图片只是跳过**
/// ——引用原样保留，由调用方汇报。一份几十个图的稿子里少一张图，不该让整次导入
/// 失败后连正文都拿不到。
pub(crate) fn import_referenced_from(markdown: &str, base: &Path) -> Result<(String, Vec<String>)> {
    let mut sources = Vec::new();
    let mut source_refs = Vec::new();
    let mut skipped = Vec::new();
    for src in image_refs(markdown) {
        if src.starts_with("http://") || src.starts_with("https://") {
            continue;
        }
        let Ok(source) = resolve_from(base, &src) else {
            skipped.push(format!("{src}（路径越出所选文件夹）"));
            continue;
        };
        if !source.is_file() {
            skipped.push(format!("{src}（文件不存在）"));
            continue;
        }
        if !is_supported(&source) {
            skipped.push(format!("{src}（格式不支持）"));
            continue;
        }
        sources.push(source);
        source_refs.push(src);
    }
    if sources.is_empty() {
        return Ok((markdown.to_string(), skipped));
    }
    let imported = import(&sources)?;
    if imported.len() != source_refs.len() {
        bail!("部分 Markdown 图片未能导入，请检查文件名和扩展名");
    }
    let replacements = source_refs
        .into_iter()
        .zip(imported)
        .map(|(source, image)| (source, image.rel_path))
        .collect::<HashMap<_, _>>();
    let rewritten = IMAGE_REF_RE
        .replace_all(markdown, |captures: &regex::Captures<'_>| {
            let original = captures.get(0).map(|m| m.as_str()).unwrap_or_default();
            let Some(source) = captures.get(1).map(|m| m.as_str()) else {
                return original.to_string();
            };
            replacements
                .get(source)
                .map(|replacement| original.replacen(source, replacement, 1))
                .unwrap_or_else(|| original.to_string())
        })
        .into_owned();
    Ok((rewritten, skipped))
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

#[cfg(test)]
pub(crate) fn copy_refs_from(base: &Path, markdown: &str, target_dir: &Path) -> Result<()> {
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

    #[test]
    fn shared_help_fallback_checks_both_the_bundled_name_and_contents() {
        let dir = tempfile::tempdir().unwrap();
        let (name, bytes) = crate::help::content::IMAGES[0];
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        assert!(public_help_copy(&path, name));
        assert!(!public_help_copy(&path, "help-private.png"));
        fs::write(&path, b"sensitive user image").unwrap();
        assert!(!public_help_copy(&path, name));
    }

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
    /// 图片目录按槽位物理隔离：真实槽 `images/`、诱饵槽 `alt-images/`，
    /// 正文引用仍统一写 `images/...`（跨槽/跨机都能读）。
    /// 启用应用锁后资源文件整份 AEAD：写密文、读回明文；失去密钥读不到，
    /// 也不能以明文覆盖已有密文（静默降级防护）。
    #[test]
    fn managed_resources_are_sealed_and_read_with_the_current_key() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let path = dir.path().join("扫描件.pdf");
        crate::vault::clear_key_handle();
        write_managed_file(&path, b"%PDF-plain").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-plain");
        assert_eq!(read_managed_file(&path).unwrap(), b"%PDF-plain");

        crate::vault::set_key_handle([4u8; 32], true);
        write_managed_file(&path, b"%PDF-secret").unwrap();
        assert!(crate::vault::is_sealed_file(&std::fs::read(&path).unwrap()));
        assert_eq!(read_managed_file(&path).unwrap(), b"%PDF-secret");

        crate::vault::clear_key_handle();
        assert!(
            read_managed_file(&path).is_err(),
            "没有密钥不能把密文当资源"
        );
        assert!(
            write_managed_file(&path, b"downgrade").is_err(),
            "没有密钥不能以明文覆盖密文"
        );
        assert!(crate::vault::is_sealed_file(&std::fs::read(&path).unwrap()));
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn managed_resource_limit_counts_plaintext_not_the_sealed_envelope() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let path = dir.path().join("boundary.bin");
        let plain = vec![42u8; 4096];
        crate::vault::set_key_handle([12u8; 32], true);
        write_managed_file(&path, &plain).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            plain.len() as u64 + crate::vault::SEALED_FILE_OVERHEAD as u64
        );
        assert_eq!(
            read_managed_file_limited(&path, plain.len() as u64, "测试资源").unwrap(),
            plain
        );

        write_managed_file(&path, &vec![42u8; 4097]).unwrap();
        assert!(read_managed_file_limited(&path, 4096, "测试资源").is_err());
        crate::vault::clear_key_handle();
        std::fs::write(&path, vec![42u8; 4097]).unwrap();
        assert!(
            read_managed_file_limited(&path, 4096, "测试资源").is_err(),
            "额外封套预算不能放宽明文文件的上限"
        );
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn app_resource_paths_reject_explicit_decoy_namespace() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        for rel in ["alt-images/secret.png", "alt-mermaid-cache/secret.svg"] {
            assert!(
                resolve_from_base(dir.path(), rel).is_err(),
                "配置目录内不得显式跨槽引用 {rel}"
            );
        }
        crate::storage::set_test_config_dir(None);

        let external = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_from_base(external.path(), "alt-images/example.png").unwrap(),
            external.path().join("alt-images/example.png"),
            "外部 Markdown 基准目录不使用应用的槽位保留名"
        );
    }

    #[test]
    fn image_dir_is_slot_aware() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        assert_eq!(
            image_dir_for(crate::storage::DataSlot::Real).unwrap(),
            dir.path().join("images")
        );
        assert_eq!(
            image_dir_for(crate::storage::DataSlot::Decoy).unwrap(),
            dir.path().join("alt-images")
        );
        assert_eq!(normalize_ref("images/扫描件.png"), "images/扫描件.png");
        crate::storage::set_test_config_dir(None);
    }
}
