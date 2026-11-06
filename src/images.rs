//! 图片资源：图片、PDF 插图与 Mermaid 图表缓存存在稿件主库的资源对象表里
//! （见 [`crate::resources`]），正文用逻辑引用 `images/<随机 ID>`。
//!
//! 设计要点：
//!
//! - **不再一图一文件**：文件名、格式、导入时间会留在磁盘目录列表里，即使内容
//!   加密也等于把「这是什么图」写在外面。进 SQLCipher 库后这些元数据都在密文里；
//!   写入与正文/版本同一个库，崩溃恢复只有一套状态，同一张图还能按 SHA-256 去重。
//! - **旧数据兼容**：升级时把 `images/<原文件名>` 解密后插进库、记成别名再删文件；
//!   读取先查别名，查不到才回退磁盘文件（帮助截图、迁移未完成、外部基准目录）。
//!   旧稿件包/旧稿件里写的就是 `images/<原文件名>`，别名保证这些引用继续有效。
//! - **新引用不含语义**：`images/<uuid>`，没有扩展名。格式一律解密后按魔数判断
//!   （[`meta`]、[`crate::resources::sniff`]），不再依赖文件名后缀。
//! - 图片必须独占一行（`![alt](src)` 一行），插入时自动在前后补空行保证；
//!   PDF 作为文件入库时预览取第一页光栅化，导出仍按原矢量嵌入。
//! - 拒绝绝对路径与 `..` 穿越，防止 markdown 引用读取任意文件。
//! - 删除稿件时按引用清理资源别名与无引用对象（`manuscript::purge_orphan_images`），
//!   仍被其他稿件或历史版本引用的资源一律保留。

use crate::app::file_dialog::FileDialogSpec;
use crate::resources;
use crate::storage;
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// 相对路径目录名（相对配置目录，也是逻辑引用前缀）。
pub(crate) const IMAGE_DIR: &str = "images";

/// 支持的图片/附件扩展名（小写、不带点）。只用于识别**外部导入文件**与旧文件回退；
/// 库里的资源按魔数判格式，不看扩展名。
pub(crate) const SUPPORTED_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "webp", "bmp", "gif", "pdf"];

/// 与解析器（export::parse_markdown_located 的独占行识别）保持一致的图片引用正则。
/// src 不含空格、换行与右括号——新引用是随机 ID，旧引用入库文件名经过净化，天然满足。
static IMAGE_REF_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"!\[[^\]\n]*\]\(\s*([^)\s]+)\s*\)").expect("图片引用正则必须合法")
});

/// 一次导入产出的图片记录。
#[derive(Debug, Clone)]
pub(crate) struct ImportedImage {
    /// 逻辑引用（新数据是 `images/<uuid>`，降级/旧夹具是 `images/<文件名>`）。
    pub rel_path: String,
    /// 可直接插入文档的 markdown 片段。
    pub markdown: String,
}

/// 一张图片的元数据（格式、原文件名）。取自资源库；旧文件按扩展名回退。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageMeta {
    pub media_type: String,
    pub original_name: String,
    /// 新数据有内容校验值；旧文件回退时为空串。
    pub sha256: String,
}

/// 图片资源目录：配置目录/images/（**槽位感知**：诱饵系统用 `alt-images/`）。
///
/// 新数据不再往这里写用户图片，目录只保留随包帮助截图与迁移中的旧文件；
/// 槽位映射仍要保留，供旧数据读取、迁移与擦除使用。
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

/// 以给定目录为基准解析相对路径（拒绝绝对路径与 `..` 穿越），供导出/备份/旧文件复用。
///
/// 配置目录是**槽位命名空间**的唯一基准：`images/` 前缀要映射到当前槽位的图片
/// 目录（真实 `images/`、诱饵 `alt-images/`），正文里的引用仍统一写
/// `images/...`，跨槽/跨机器都能读。样张与测试传进来的临时基准目录保持
/// 原样 join，不参与槽位映射。
pub(crate) fn resolve_from_base(base: &Path, rel: &str) -> Result<PathBuf> {
    let path = Path::new(rel);
    validate_relative(path, rel)?;
    if is_config_base(base) {
        return app_relative(path);
    }
    Ok(base.join(path))
}

/// 基准目录是不是当前配置目录（资源库与槽位命名空间的唯一入口）。
fn is_config_base(base: &Path) -> bool {
    storage::config_dir().is_ok_and(|dir| dir == base)
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
        // 旧版图表缓存的物理目录；新缓存进资源库，这里只服务迁移前的文件回退。
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

/// 读取应用管理的**文件**（旧图片、旧图表缓存、帮助截图）。新资源进库后由
/// [`read`] 处理；这个入口保留给迁移、导出中间文件与公开帮助图。
pub(crate) fn read_managed_file(path: &Path) -> Result<Vec<u8>> {
    read_managed_file_limited(path, crate::file_limits::MAX_IMAGE_BYTES, "资源文件")
}

/// 带体积上限的 [`read_managed_file`]：`max_bytes` 始终限制解密后的明文；
/// 原始读取额外允许一个固定封套，未密封文件仍按原上限拒绝。
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

/// 写入应用管理的**文件**。新数据已改走资源库；这里保留给帮助截图释放、
/// 迁移回滚与测试夹具。有句柄时密封，没有句柄时写明文（未启用应用锁的系统）；
/// 若目标已是密文则拒绝写明文，避免静默降级。
pub(crate) fn write_managed_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let kind = if crate::vault::key_handle().is_some() {
        crate::vault::resource_kind_for_path(path)?
    } else {
        "image"
    };
    crate::vault::write_path(path, kind, bytes)
}

/// 随包公开的帮助截图（名称与字节都与内置资源一致）：属于公开内容，不做槽位复制，
/// 也不参与资源入库与加密迁移；诱饵槽可安全回落到真实槽的共享副本。
pub(crate) fn is_public_help_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| public_help_copy(path, name))
}

/// 可丢弃文件的写入（旧图表缓存回退路径；新缓存进库）：有句柄时密封并允许覆盖
/// 当前密钥解不开的旧密文，没有句柄时写明文。
pub(crate) fn write_disposable_managed_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let kind = if crate::vault::key_handle().is_some() {
        crate::vault::resource_kind_for_path(path)?
    } else {
        "image"
    };
    crate::vault::write_path_replacing(path, kind, bytes)
}

/// 读一份图片资源：先查资源库别名，没有别名再回退磁盘文件（旧数据/帮助图）。
pub(crate) fn read(src: &str) -> Result<Vec<u8>> {
    read_limited_from_base(
        &storage::config_dir()?,
        src,
        crate::file_limits::MAX_IMAGE_BYTES,
    )
}

/// 以指定基准目录读图片资源；配置目录基准优先查库，外部目录只读文件。
pub(crate) fn read_from_base(base: &Path, src: &str) -> Result<Vec<u8>> {
    read_limited_from_base(base, src, crate::file_limits::MAX_IMAGE_BYTES)
}

/// 带明文体积上限的 [`read_from_base`]：库里的对象先看 `plain_size` 再读，
/// 旧文件按解密后的明文卡同一上限。
pub(crate) fn read_limited_from_base(base: &Path, src: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let logical = normalize_ref(src);
    if is_config_base(base) {
        if let Some(meta) = resources::meta(&logical)?
            && meta.plain_size > max_bytes
        {
            return Err(crate::file_limits::too_large("图片", max_bytes).into());
        }
        if let Some(bytes) = resources::get(&logical)? {
            if bytes.len() as u64 > max_bytes {
                return Err(crate::file_limits::too_large("图片", max_bytes).into());
            }
            return Ok(bytes);
        }
    }
    let path = resolve_from_base(base, &logical)?;
    read_managed_file_limited(&path, max_bytes, "图片")
}

/// 图片资源是否存在（库别名或回退文件），供导出时判断「缺图跳过」。
pub(crate) fn exists(src: &str) -> bool {
    let Ok(config) = storage::config_dir() else {
        return false;
    };
    exists_from_base(&config, src)
}

/// 以指定基准目录判断资源是否存在。
pub(crate) fn exists_from_base(base: &Path, src: &str) -> bool {
    let logical = normalize_ref(src);
    if is_config_base(base) && resources::has(&logical).unwrap_or(false) {
        return true;
    }
    resolve_from_base(base, &logical).is_ok_and(|path| path.is_file())
}

/// 资源元数据：库里取 MIME 与原文件名；旧文件按扩展名回退。
pub(crate) fn meta(src: &str) -> Option<ImageMeta> {
    let logical = normalize_ref(src);
    if let Ok(Some(meta)) = resources::meta(&logical) {
        return Some(ImageMeta {
            media_type: meta.media_type,
            original_name: meta.original_name,
            sha256: meta.sha256,
        });
    }
    let name = logical.rsplit('/').next()?.to_string();
    if name.is_empty() {
        return None;
    }
    Some(ImageMeta {
        media_type: media_type_from_name(&name).to_string(),
        original_name: name,
        sha256: String::new(),
    })
}

/// 导出时给人类看的文件名（DOCX 里「【附件】扫描件.pdf」）。库里存原名；
/// 旧文件用路径末段。
pub(crate) fn display_name(src: &str) -> String {
    meta(src).map_or_else(
        || src.rsplit('/').next().unwrap_or(src).to_string(),
        |meta| meta.original_name,
    )
}

/// 这张图是不是 PDF 附件/插图（决定预览光栅化还是位图渲染、DOCX 附件说明）。
pub(crate) fn is_pdf(src: &str) -> bool {
    meta(src).is_some_and(|meta| meta.media_type == "application/pdf")
}

/// 资源内容指纹：资源库用内容 SHA-256 前 64 位，旧文件用修改时间纳秒。
/// 预览缓存按它失效（同一路径换了对象就换纹理）。
pub(crate) fn stamp(src: &str) -> u64 {
    if let Ok(Some(meta)) = resources::meta(&normalize_ref(src)) {
        let hex = meta.sha256.get(..16).unwrap_or(&meta.sha256);
        if let Ok(value) = u64::from_str_radix(hex, 16) {
            return value;
        }
    }
    resolve(src)
        .ok()
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

/// 按扩展名猜 MIME（只用于没有入库的旧文件与外部基准目录）。
fn media_type_from_name(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

/// 把一个（外部导入的）文件复制入库，返回逻辑引用。
///
/// 新引用是 `images/<uuid>`：不含原文件名、格式与时间。降级路径（资源库未初始化，
/// 例如测试夹具或库打不开）仍按旧的文件命名写到槽位目录。
pub(crate) fn store_import(name: &str, bytes: &[u8]) -> Result<String> {
    let logical = format!("{IMAGE_DIR}/{}", uuid::Uuid::new_v4().simple());
    let original_name = sanitize_original_name(name);
    store_at(&logical, &original_name, bytes)?;
    Ok(logical)
}

/// 把一份内容写到指定逻辑引用下。库可用时进资源库（同 SHA 去重），不可用时回退
/// 旧的文件写法（旧命名/测试夹具）。迁移与稿件包导入会显式给逻辑引用。
pub(crate) fn store_at(logical: &str, original_name: &str, bytes: &[u8]) -> Result<()> {
    if resources::put(logical, original_name, None, bytes)? {
        return Ok(());
    }
    // 降级：资源库不可用。仍按旧路径写文件，有密钥时 GWA2 密封。
    let path = resolve(logical)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建图片目录 {}", parent.display()))?;
    }
    write_managed_file(&path, bytes).with_context(|| format!("无法写入图片 {}", path.display()))
}

/// 写一份文本资源（Mermaid 图表警告这类没有魔数可认的内容）。库可用时进资源库，
/// 否则回退旧的文件写法。
pub(crate) fn store_text_at(logical: &str, original_name: &str, bytes: &[u8]) -> Result<()> {
    if resources::put(logical, original_name, Some("text/plain"), bytes)? {
        return Ok(());
    }
    let path = resolve(logical)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建资源目录 {}", parent.display()))?;
    }
    write_disposable_managed_file(&path, bytes)
}

/// 旧的文件写法入口（测试夹具）：把一份资源按当前槽位/密钥写进给定文件路径。
#[cfg(test)]
pub(crate) fn write_image_file(path: &Path, bytes: &[u8]) -> Result<()> {
    write_managed_file(path, bytes)
}

/// 丢弃一条刚写入的资源（导入中途失败回滚）。
fn discard_stored(logical: &str) {
    let _ = resources::remove_alias(logical);
    let _ = resources::gc_objects();
    if let Ok(path) = resolve(logical) {
        let _ = fs::remove_file(path);
    }
}

/// "插入图片"的文件选择框参数（可多选）。对话框由应用层在后台线程开。
pub(crate) fn pick_files_spec() -> FileDialogSpec {
    let bitmaps = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];
    FileDialogSpec::pick_files()
        .filter("图片 / PDF", SUPPORTED_EXTENSIONS)
        .filter("PNG / JPG / WebP / BMP / GIF", &bitmaps)
        .filter("PDF 附件", &["pdf"])
}

/// 把选中文件复制入库，返回每个文件的逻辑引用与 markdown 片段。
/// 不支持扩展名的文件被跳过；空输入返回空列表。
pub(crate) fn import(files: &[PathBuf]) -> Result<Vec<ImportedImage>> {
    let mut out = Vec::new();
    let mut created: Vec<String> = Vec::new();
    let result = (|| -> Result<()> {
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
            let rel_path = store_import(name, &bytes)?;
            created.push(rel_path.clone());
            let alt = sanitize_alt(
                Path::new(name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            let markdown = format!("![{alt}]({rel_path})");
            out.push(ImportedImage { rel_path, markdown });
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(out),
        Err(error) => {
            for logical in created {
                discard_stored(&logical);
            }
            Err(error)
        }
    }
}

/// 把内存里的图片字节入库，返回逻辑引用，顺序与输入一致。
/// 供文档导入与富文本粘贴用：Word 等文档里内嵌的图片由转换器取出，没有现成文件。
/// 任一张写失败时本批已写入的引用全部删掉。
pub(crate) fn import_bytes(items: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut refs = Vec::new();
    let mut created: Vec<String> = Vec::new();
    let result = (|| -> Result<()> {
        for (name, bytes) in items {
            let logical = store_import(name, bytes)?;
            created.push(logical.clone());
            refs.push(logical);
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(refs),
        Err(error) => {
            for logical in created {
                discard_stored(&logical);
            }
            Err(error)
        }
    }
}

/// 旧的文件写法：把图片字节写进给定目录（测试夹具），返回相对路径。
#[cfg(test)]
fn store_into_dir(
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

/// 测试夹具：把外部文件按旧命名写进指定目录。
#[cfg(test)]
fn import_into(dir: &Path, files: &[PathBuf]) -> Result<Vec<ImportedImage>> {
    fs::create_dir_all(dir).with_context(|| format!("无法创建图片目录 {}", dir.display()))?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
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
            let rel_path = store_into_dir(dir, &stamp, name, &bytes, &mut created)?;
            let alt = sanitize_alt(
                Path::new(name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            out.push(ImportedImage {
                rel_path: rel_path.clone(),
                markdown: format!("![{alt}]({rel_path})"),
            });
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

/// 测试夹具：把内存字节按旧命名写进指定目录，返回相对路径。
#[cfg(test)]
pub(crate) fn import_bytes_into(dir: &Path, items: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(dir).with_context(|| format!("无法创建图片目录 {}", dir.display()))?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let mut created = Vec::new();
    let result = items
        .iter()
        .map(|(name, bytes)| store_into_dir(dir, &stamp, name, bytes, &mut created))
        .collect::<Result<Vec<_>>>();
    if result.is_err() {
        for path in created {
            let _ = fs::remove_file(path);
        }
    }
    result
}

/// 迁移统计。`skipped` 是留在磁盘上的文件与原因（不会静默丢数据）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct MigrateReport {
    /// 已并入资源库并删除的旧图片数。
    pub migrated: usize,
    /// 已删除的可重建图表缓存文件数。
    pub removed_cache: usize,
    /// 没能并入、仍按文件保留的条目（含原因）。
    pub skipped: Vec<String>,
}

/// 把槽位目录里的旧图片文件并入资源库并安全删除；可重建的 Mermaid 缓存直接清掉。
///
/// 幂等：中断后重跑会跳过已入库的同内容文件、继续处理剩下的。只在资源库可用时
/// 执行；入不了库（内容认不出、库里同名不同内容）的文件保留并报告，不误删。
pub(crate) fn migrate_legacy_files() -> Result<MigrateReport> {
    let mut report = MigrateReport::default();
    if !resources::available() {
        return Ok(report);
    }
    let dir = image_dir()?;
    if dir.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
            .with_context(|| format!("无法读取图片目录 {}", dir.display()))?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        for path in entries {
            if !path.is_file() {
                continue;
            }
            let Some(name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                report
                    .skipped
                    .push(format!("{}：文件名不是 UTF-8", path.display()));
                continue;
            };
            if is_public_help_file(&path) {
                continue;
            }
            let logical = format!("{IMAGE_DIR}/{name}");
            let bytes = match read_managed_file(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    report.skipped.push(format!("{name}：{error:#}"));
                    continue;
                }
            };
            let sha256 = crate::manuscript::sync::bytes_hash(&bytes);
            match resources::meta(&logical)? {
                Some(meta) if meta.sha256 == sha256 => {
                    // 上次迁移到一半（别名已写、文件没删）：直接删文件。
                    crate::vault::overwrite_then_delete(&path);
                    report.migrated += 1;
                }
                Some(_) => {
                    report.skipped.push(format!(
                        "{name}：资源库已有同名引用但内容不同，保留文件不覆盖"
                    ));
                }
                None => match resources::put(&logical, &name, None, &bytes) {
                    Ok(true) => {
                        // 先把已提交内容并回主库文件，再删原文件：WAL 未落盘时
                        // 断电不能出现「库里没有、原文件也没了」。
                        resources::checkpoint()?;
                        crate::vault::overwrite_then_delete(&path);
                        report.migrated += 1;
                    }
                    Ok(false) => {
                        report
                            .skipped
                            .push(format!("{name}：资源库不可用，保留文件"));
                        break;
                    }
                    Err(error) => {
                        report.skipped.push(format!("{name}：{error:#}"));
                    }
                },
            }
        }
    }
    // Mermaid 缓存是可重建的：旧样式/旧主题的图没有保留价值，直接删；新缓存进库。
    if let Ok(cache) = storage::slot_subdir(storage::MERMAID_CACHE_DIR)
        && cache.is_dir()
    {
        for entry in fs::read_dir(&cache)? {
            let path = entry?.path();
            if path.is_file() && fs::remove_file(&path).is_ok() {
                report.removed_cache += 1;
            }
        }
        // 目录空了就删掉，别在配置目录里留一个空壳。
        let _ = fs::remove_dir(&cache);
    }
    Ok(report)
}

/// 把外部 Markdown 引用的本地图片复制入资源库并改写引用，返回改写后的
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
        let Ok(source) = resolve_from_base(base, &src) else {
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
        .collect::<std::collections::HashMap<_, _>>();
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

/// 净化原文件名（只作展示用，不再落盘）：剔路径分隔符、控制字符与 markdown 特殊字符。
fn sanitize_original_name(name: &str) -> String {
    let out: String = name
        .chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect();
    if out.trim().is_empty() {
        "image".into()
    } else {
        out
    }
}

/// 净化文件名：剔除路径分隔符、控制字符、Windows 非法字符与 markdown 特殊字符，
/// 保留扩展名。仅旧文件写法（测试夹具/降级路径）使用。
#[cfg(test)]
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

/// 是否支持该扩展名（只用于外部导入文件与旧数据）。
pub(crate) fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.as_str()))
}

/// 规范化 markdown 图片引用：统一分隔符为 `/`、去掉 `./` 前缀，让手写引用
/// （`images\a.png`、`./images/a.png`）与库引用可以互相比较，避免孤儿清理时
/// 把同义的引用误判为不同而漏删对象。
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

/// 旧测试夹具：从基准目录按文件复制插图到目标目录（保留相对布局）。
#[cfg(test)]
pub(crate) fn copy_refs_from(base: &Path, markdown: &str, target_dir: &Path) -> Result<()> {
    for src in image_refs(markdown) {
        let source = match resolve_from_base(base, &src) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;
    use std::fs::File;
    use std::io::Write as _;

    fn write_tmp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut f = File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    /// 测试用完必须复位全局资源库与配置目录覆盖，避免串到别的测试。
    struct Fixture {
        /// 只为让临时目录活到测试结束；内容由 `resources` 与文件回退读取。
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            storage::set_test_config_dir(Some(dir.path().to_path_buf()));
            resources::init_at(&dir.path().join("manuscripts.db")).unwrap();
            Self { _dir: dir }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            resources::close();
            storage::set_test_config_dir(None);
        }
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
    fn import_copies_files_into_the_legacy_dir_helper() {
        // 旧的 `import_into` 只给测试夹具与降级路径用：仍按文件命名写进指定目录。
        let tmp = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let png = write_tmp(src.path(), "扫描 件.png", b"\x89PNG\r\n\x1a\nfake");
        let pdf = write_tmp(src.path(), "附件.pdf", b"%PDF-1.4");
        let txt = write_tmp(src.path(), "说明.txt", b"skip me");
        let dir = tmp.path().join(IMAGE_DIR);
        let imgs = import_into(&dir, &[png.clone(), pdf, txt]).unwrap();
        assert_eq!(imgs.len(), 2);
        for img in &imgs {
            assert!(img.rel_path.starts_with("images/"));
            assert!(img.rel_path.ends_with("_扫描_件.png") || img.rel_path.ends_with("_附件.pdf"));
            assert!(img.markdown.starts_with("!["));
        }
    }

    #[test]
    fn imported_images_go_into_the_resource_database_without_semantic_names() {
        let fixture = Fixture::new();
        let png = b"\x89PNG\r\n\x1a\ncontent".to_vec();
        let refs = import_bytes(&[("关于XX的请示_图1.png".into(), png.clone())]).unwrap();
        assert_eq!(refs.len(), 1);
        let logical = &refs[0];
        assert!(logical.starts_with("images/"));
        // 新引用不含原文件名、时间戳与扩展名。
        assert!(!logical.contains("请示"));
        assert!(!logical.contains(".png"));
        // 槽位图片目录里没有生成文件。
        let dir = image_dir().unwrap();
        assert!(!dir.exists() || fs::read_dir(&dir).unwrap().next().is_none());
        // 读回与元数据：内容在库，原名只在库里。
        assert_eq!(read(logical).unwrap(), png);
        let meta = meta(logical).unwrap();
        assert_eq!(meta.media_type, "image/png");
        assert_eq!(meta.original_name, "关于XX的请示_图1.png");
        assert!(!is_pdf(logical));
        assert!(display_name(logical).contains("请示"));
        drop(fixture);
    }

    #[test]
    fn pdf_illustrations_are_recognized_by_bytes_not_extension() {
        let fixture = Fixture::new();
        let pdf = b"%PDF-1.7 illustration".to_vec();
        let logical = store_import("插图.pdf", &pdf).unwrap();
        assert!(is_pdf(&logical), "库里按魔数认 PDF");
        assert_eq!(read(&logical).unwrap(), pdf);
        drop(fixture);
    }

    #[test]
    fn dedupes_identical_content_but_keeps_distinct_aliases() {
        let fixture = Fixture::new();
        let png = b"\x89PNG\r\n\x1a\nsame".to_vec();
        let a = store_import("a.png", &png).unwrap();
        let b = store_import("b.png", &png).unwrap();
        assert_ne!(a, b, "两次导入各有自己的逻辑引用");
        assert_eq!(read(&a).unwrap(), read(&b).unwrap());
        assert_eq!(
            resources::all_aliases_with(&fixture_db(&fixture))
                .unwrap()
                .len(),
            2
        );
        drop(fixture);
    }

    /// 打开与资源库同一个库文件的独立连接，供测试直接查表。
    fn fixture_db(_fixture: &Fixture) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(storage::manuscript_db_path().unwrap()).unwrap();
        resources::ensure_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn legacy_files_are_folded_into_the_database_and_removed() {
        let fixture = Fixture::new();
        let dir = image_dir().unwrap();
        fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("20260101_扫描件.png");
        fs::write(&legacy, b"\x89PNG\r\n\x1a\nlegacy").unwrap();
        let report = migrate_legacy_files().unwrap();
        assert_eq!(report.migrated, 1);
        assert!(!legacy.exists(), "迁移后旧文件应安全删除");
        let logical = "images/20260101_扫描件.png";
        assert_eq!(read(logical).unwrap(), b"\x89PNG\r\n\x1a\nlegacy");
        assert_eq!(meta(logical).unwrap().original_name, "20260101_扫描件.png");
        // 幂等：再跑一次没有可迁移的文件。
        assert_eq!(migrate_legacy_files().unwrap().migrated, 0);
        drop(fixture);
    }

    #[test]
    fn legacy_mermaid_cache_is_removed_not_kept() {
        let fixture = Fixture::new();
        let cache = storage::slot_subdir(storage::MERMAID_CACHE_DIR).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("gongwen-mermaid-canvas-abc.png"), b"sealed").unwrap();
        let report = migrate_legacy_files().unwrap();
        assert_eq!(report.removed_cache, 1);
        assert!(!cache.exists());
        drop(fixture);
    }

    #[test]
    fn fallback_file_store_is_used_when_the_resource_database_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        resources::close();
        let logical = store_import("扫描件.png", b"\x89PNG\r\n\x1a\nfallback").unwrap();
        // 资源库不可用时按旧路径写文件，仍能读回；引用仍是 images/ 前缀。
        assert!(logical.starts_with("images/"));
        assert_eq!(read(&logical).unwrap(), b"\x89PNG\r\n\x1a\nfallback");
        storage::set_test_config_dir(None);
    }

    #[test]
    fn resolve_rejects_escape_and_accepts_relative() {
        let base = Path::new("/base");
        assert_eq!(
            resolve_from_base(base, "images/a.png").unwrap(),
            PathBuf::from("/base/images/a.png")
        );
        assert!(resolve_from_base(base, "../a.png").is_err());
        assert!(resolve_from_base(base, "images/../../a.png").is_err());
        assert!(resolve_from_base(base, "/etc/passwd").is_err());
        #[cfg(windows)]
        assert!(resolve_from_base(base, "C:\\evil.png").is_err());
    }

    #[test]
    fn image_refs_scans_and_deduplicates() {
        let md = "![a](images/1.png)\n正文\n![b](images/2.pdf)\n![重复](images/1.png)";
        assert_eq!(image_refs(md), vec!["images/1.png", "images/2.pdf"]);
        assert!(image_refs("没有图片").is_empty());
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
        assert!(!target.path().join("images/c.png").exists());
        copy_refs_from(base.path(), "![x](../etc/passwd)", target.path()).unwrap();
    }

    #[test]
    fn image_dir_is_slot_aware() {
        let dir = tempfile::tempdir().unwrap();
        storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        assert_eq!(
            image_dir_for(storage::DataSlot::Real).unwrap(),
            dir.path().join("images")
        );
        assert_eq!(
            image_dir_for(storage::DataSlot::Decoy).unwrap(),
            dir.path().join("alt-images")
        );
        storage::set_test_config_dir(None);
    }
}
