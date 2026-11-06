//! 稿件包导出/导入：新写文件名加密的 7z，兼容读取旧 ZIP。
//!
//! 包内布局：加密清单、按稿件 UUID 命名的 PDF 附件和带校验值的图片资源。
//! 导出按筛选条件或勾选稿件；导入由逐篇身份判定和用户选定的动作决定。

#[cfg(test)]
use crate::manuscript::ManuscriptFilter;
#[cfg(test)]
use crate::manuscript::NewManuscript;
use crate::manuscript::send_package::{SendPackageSnapshot, is_owner_kind};
use crate::manuscript::sync::{SyncRevision, bytes_hash};
use crate::manuscript::{ManuscriptRecord, ManuscriptStore};
use crate::models::{
    DraftInput, FontConfig, ManuscriptStatus, NumberingConfig, TemplateKind, VocabularyEntry,
};
use crate::units::UnitDisplay;
use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::fs::File;
#[cfg(test)]
use std::io::Write;
use std::path::Path;
#[cfg(test)]
use zip::write::SimpleFileOptions;
#[cfg(test)]
use zip::{AesMode, ZipWriter};

mod archive;
use archive::{PackageReader, PackageWriter};

pub mod send_package;
pub mod sync;

pub const MANIFEST_SCHEMA: u32 = 3;
const MANIFEST_NAME: &str = "manifests.json";
/// 随稿件包导出的标准词库（全局一份，随包带走，导入时增量合并）。
pub const VOCABULARY_SCHEMA: u32 = 2;
const VOCABULARY_NAME: &str = "vocabulary.json";
/// 词库 JSON 体积上限（约 10 MB），远超正常词库规模，防恶意包撑爆内存。
const VOCABULARY_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 单个 PDF 附件上限（约 100 MB），超限跳过，避免导入超大文件撑爆库。
const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;
/// 单张图片的上限（解压后），与插图入库同一个值，插得进去的图就带得回来。
const MAX_IMAGE_BYTES: u64 = crate::file_limits::MAX_IMAGE_BYTES;
/// 一次导入要读进内存的图片合计上限（解压后）：防 zip 炸弹，又给大稿件库
/// 留足余量。
const MAX_IMAGES_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const MANIFEST_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// 导出密码规则：与应用锁共用同一套判据（`crate::password_policy`）——
/// 不少于 10 个字符，且至少覆盖三类字符；同时拒绝常见口令、
/// 连续字符和长重复字符。导入不调用此校验，以兼容外部工具生成的历史弱密码包。
pub fn validate_export_password(password: &str) -> Result<()> {
    crate::password_policy::validate_export(password)
}

#[cfg(test)]
fn encrypted_options(password: &str) -> zip::write::FileOptions<'_, ()> {
    SimpleFileOptions::default().with_aes_encryption(AesMode::Aes256, password)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPdf {
    pub id: i64,
    pub file_name: String,
    /// zip 内相对路径，如 `pdf/3_0_扫描件.pdf`。
    pub path: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestRecord {
    /// 旧格式保留的来源库整数 id；新版仅供兼容展示，不参与稿件身份判定。
    pub id: i64,
    pub title: String,
    pub kind: TemplateKind,
    pub status: ManuscriptStatus,
    pub doc_number: String,
    pub doc_date: String,
    pub notes: String,
    pub content_markdown: String,
    pub snapshot: DraftInput,
    pub created_at: String,
    pub updated_at: String,
    pub published_at: Option<String>,
    pub archived_at: Option<String>,
    pub pdfs: Vec<ManifestPdf>,
    #[serde(default)]
    pub document_uuid: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub head_revision_uuid: Option<String>,
    #[serde(default)]
    pub revisions: Vec<SyncRevision>,
    /// 主件（呈批件）的送批材料清单与钉版。字段缺失 = 旧版程序导出的包，导入时
    /// 不动本机清单；显式的空清单才表示「对方把清单清空了」。为空时不写出，旧包记录的
    /// 序列化结果不变，旧包导入的指纹去重不受影响。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_package: Option<SendPackageSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestAsset {
    pub path: String,
    pub sha256: String,
}

/// zip 内相对路径 + 文件字节。
type Blob = (String, Vec<u8>);

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub exported_at: String,
    pub records: Vec<ManifestRecord>,
    #[serde(default)]
    pub assets: Vec<ManifestAsset>,
}

/// 随稿件包携带的标准词库。可选条目：旧包没有 `vocabulary.json` 时导入端视为无词库。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VocabularyFile {
    pub schema: u32,
    pub exported_at: String,
    pub entries: Vec<VocabularyEntry>,
}

#[derive(Debug)]
pub struct ExportSummary {
    pub records: usize,
    pub pdfs: usize,
}

/// 稿件 PDF 批量导出的选项：盖章件直接取附件，非盖章件编译 TeX 生成。
#[derive(Debug, Clone, Copy)]
pub struct PdfExportOptions {
    /// 导出盖章件：直接取自稿件附件里的 PDF。
    pub stamped: bool,
    /// 导出非盖章件：编译 TeX 生成 PDF。
    pub compiled: bool,
}

/// 稿件 PDF 批量导出的汇总：成功数 + 逐篇失败原因（不阻断整批）。
#[derive(Debug, Default)]
pub struct PdfExportSummary {
    /// 处理的稿件数。
    pub records: usize,
    /// 成功写入 zip 的 PDF 文件数。
    pub pdfs: usize,
    /// 未导出的稿件：`(标题, 原因)`。
    pub failed: Vec<(String, String)>,
}

#[cfg(test)]
#[derive(Debug, Default)]
pub struct ImportSummary {
    pub imported: usize,
    pub skipped_existing: usize,
    pub pdfs_imported: usize,
    /// 附件在 zip 中缺失或超出大小上限，跳过但不中断整批导入。
    pub skipped_pdfs: usize,
}

#[cfg(test)]
#[derive(Debug)]
pub struct ImportOptions {
    pub skip_existing_by_id: bool,
    /// 与 manifest.records 等长；true 才导入。
    pub selected: Vec<bool>,
}

/// 按过滤条件导出稿件（含 PDF 附件）为 7z。`vocabulary` 为标准词库，非空时随包导出，
/// 便于把稿件带到另一台电脑后保持要素一致。没有符合条件稿件时直接报错。
/// `include_send_package` 为真时，所选呈批件的随行件（送批材料）一并打包。
#[cfg(test)]
pub fn export_zip(
    store: &mut ManuscriptStore,
    filter: &ManuscriptFilter,
    vocabulary: &[VocabularyEntry],
    zip_path: &Path,
    password: &str,
    include_send_package: bool,
) -> Result<ExportSummary> {
    validate_export_password(password)?;
    let rows = store.list(filter)?;
    if rows.is_empty() {
        bail!("没有符合过滤条件的稿件");
    }
    let mut ids = rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
    if include_send_package {
        ids = store.with_send_package_items(&ids)?;
    }
    export_zip_ids(store, &ids, vocabulary, zip_path, password)
}

/// 只导出稿件管理页明确勾选的记录。
/// 只导出稿件管理页明确勾选的记录；`include_send_package` 同 [`export_zip`]。
#[cfg(test)]
pub fn export_zip_selected(
    store: &mut ManuscriptStore,
    ids: &[i64],
    vocabulary: &[VocabularyEntry],
    zip_path: &Path,
    password: &str,
    include_send_package: bool,
) -> Result<ExportSummary> {
    validate_export_password(password)?;
    let ids = if include_send_package {
        store.with_send_package_items(ids)?
    } else {
        ids.to_vec()
    };
    export_zip_ids(store, &ids, vocabulary, zip_path, password)
}

#[cfg(test)]
fn export_zip_ids(
    store: &mut ManuscriptStore,
    ids: &[i64],
    vocabulary: &[VocabularyEntry],
    zip_path: &Path,
    password: &str,
) -> Result<ExportSummary> {
    prepare_package(store, ids, vocabulary)?.write(zip_path, password)
}

/// 主线程固定稿件、历史、附件与词库快照；后台只编码这一份内存快照。
/// 避免 7z 密码派生占住 GUI，也避免导出中途编辑混入本次稿件包。
pub(crate) struct PreparedPackage {
    manifest: Manifest,
    vocabulary: Vec<VocabularyEntry>,
    blobs: Vec<Blob>,
}

pub(crate) fn prepare_package(
    store: &mut ManuscriptStore,
    ids: &[i64],
    vocabulary: &[VocabularyEntry],
) -> Result<PreparedPackage> {
    if ids.is_empty() {
        bail!("没有选中要导出的稿件");
    }
    let mut records = Vec::new();
    let mut pdf_blobs: Vec<(String, Vec<u8>)> = Vec::new();
    for &id in ids {
        let head = store.sync_checkpoint(id)?;
        let Some(record) = store.get(id)? else {
            continue;
        };
        let (document_uuid, _) = store.document_identity(id)?;
        let revisions = store.sync_revisions(id)?;
        let aliases = store.document_aliases(id)?;
        let export_id = record.source_id.unwrap_or(record.id);
        // 呈批件一律带上清单（哪怕是空的）：对方据此知道这边把清单清空了。
        // 改过文种但还挂着随行件的也带，免得清单在同步中丢掉。
        let send_package = if is_owner_kind(record.kind) || store.has_send_package_items(id)? {
            Some(store.send_package_snapshot(id)?)
        } else {
            None
        };
        let mut pdfs = Vec::new();
        for (idx, pdf) in record.pdfs.iter().enumerate() {
            let entry = format!(
                "pdf/{document_uuid}_{idx}_{}",
                sanitize_entry_name(&pdf.file_name)
            );
            pdf_blobs.push((entry.clone(), pdf.bytes.clone()));
            pdfs.push(ManifestPdf {
                id: pdf.id,
                file_name: pdf.file_name.clone(),
                path: entry,
                sha256: Some(bytes_hash(&pdf.bytes)),
            });
        }
        records.push(ManifestRecord {
            id: export_id,
            title: record.title,
            kind: record.kind,
            status: record.status,
            doc_number: record.doc_number,
            doc_date: record.doc_date,
            notes: record.notes,
            content_markdown: record.content_markdown,
            snapshot: record.snapshot,
            created_at: record.created_at,
            updated_at: record.updated_at,
            published_at: record.published_at,
            archived_at: record.archived_at,
            pdfs,
            document_uuid: Some(document_uuid),
            aliases,
            head_revision_uuid: Some(head.revision_uuid),
            revisions,
            send_package,
        });
    }
    // 全部历史版本引用的图片也必须随包保存，不能只收活稿。
    let (assets, image_blobs) = collect_sync_images(&records)?;
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA,
        exported_at: Local::now().to_rfc3339(),
        records,
        assets,
    };

    Ok(PreparedPackage {
        manifest,
        vocabulary: vocabulary.to_vec(),
        blobs: pdf_blobs.into_iter().chain(image_blobs).collect(),
    })
}

impl PreparedPackage {
    pub(crate) fn write(self, zip_path: &Path, password: &str) -> Result<ExportSummary> {
        validate_export_password(password)?;
        let Self {
            manifest,
            vocabulary,
            blobs,
        } = self;
        let parent = zip_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("无法在目标目录创建临时稿件包：{}", parent.display()))?;
        let mut archive = PackageWriter::new(temporary.reopen()?, password)?;
        archive.write(MANIFEST_NAME, &serde_json::to_vec_pretty(&manifest)?, true)?;
        // 词库非空才随包：与旧包相同的清单，不改变版本图与词库身份。
        if !vocabulary.is_empty() {
            let vocab_file = VocabularyFile {
                schema: VOCABULARY_SCHEMA,
                exported_at: Local::now().to_rfc3339(),
                entries: vocabulary,
            };
            archive.write(
                VOCABULARY_NAME,
                &serde_json::to_vec_pretty(&vocab_file)?,
                true,
            )?;
        }
        // 图片与 PDF 已压缩，只加密；不生成明文中间文件。
        for (path, bytes) in &blobs {
            archive.write(path, bytes, false)?;
        }
        let finished = archive.finish()?;
        finished.sync_all()?;
        drop(finished);
        temporary
            .persist(zip_path)
            .map_err(|error| error.error)
            .with_context(|| format!("无法完成稿件包：{}", zip_path.display()))?;
        Ok(ExportSummary {
            records: manifest.records.len(),
            pdfs: manifest
                .records
                .iter()
                .map(|record| record.pdfs.len())
                .sum(),
        })
    }
}

/// 把选中的稿件按选项导出为 PDF 集合，压缩进一个文件名加密的 7z。
///
/// - 盖章件（`stamped`）：直接取稿件附件里的 PDF；
/// - 非盖章件（`compiled`）：在进程内用 Typst 排版，直接取得内存 PDF 字节。
///
/// 文件按稿件导出命名主干（不含分钟级时间戳）命名，盖章件在主干后追加
/// `（盖章）`（同一稿件多个盖章附件依次 `（盖章2）`、`（盖章3）`…）。逐篇
/// 失败不阻断整批，结果汇总到 `PdfExportSummary`；`progress` 用于回报进度。
#[allow(clippy::too_many_arguments)] // 数据源、导出选项、加密目标和进度回调均是独立职责。
pub fn export_selected_pdfs(
    store: &mut ManuscriptStore,
    ids: &[i64],
    options: &PdfExportOptions,
    vocabulary: &[VocabularyEntry],
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    zip_path: &Path,
    password: &str,
    mut progress: impl FnMut(&str),
) -> Result<PdfExportSummary> {
    validate_export_password(password)?;
    if ids.is_empty() {
        bail!("没有选中要导出的稿件");
    }
    if !options.stamped && !options.compiled {
        bail!("请至少选择盖章件或非盖章件");
    }
    // 先写临时文件，成功后再原子改名到目标路径：中途失败（磁盘满、DB 错误等）
    // 不会留下损坏的 zip，也不会截断用户已有的旧压缩包。
    let parent = zip_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut archive = PackageWriter::new(temporary.reopen()?, password)?;
        let mut used_names = std::collections::HashSet::new();
        let mut summary = PdfExportSummary::default();
        let display = UnitDisplay::new(vocabulary);
        let (fonts, _warnings) = crate::system_fonts::resolve(fonts);
        let total = ids.len();
        for &id in ids {
            summary.records += 1;
            let Some(record) = store.get(id)? else {
                summary
                    .failed
                    .push((format!("（稿件 id={id}）"), "记录不存在，已跳过".into()));
                progress(&format!("已完成 {}/{} 篇", summary.records, total));
                continue;
            };
            let title = crate::export::document_title(&record.snapshot, &record.content_markdown);
            let stem = crate::export::document_stem_prefix(&record.snapshot, &title);
            let label = record.title.clone();
            let missing_stamp = options.stamped && record.pdfs.is_empty();
            let mut wrote = 0usize;
            if options.stamped {
                for (idx, pdf) in record.pdfs.iter().enumerate() {
                    let name = if idx == 0 {
                        format!("{stem}（盖章）")
                    } else {
                        format!("{stem}（盖章{}）", idx + 1)
                    };
                    let entry = unique_zip_name(&mut used_names, &name, "pdf");
                    archive.write(&entry, &pdf.bytes, false)?;
                    wrote += 1;
                }
            }
            if options.compiled {
                match compile_record_pdf(&record, &display, &fonts, numbering) {
                    Ok(pdf_bytes) => {
                        let entry = unique_zip_name(&mut used_names, &stem, "pdf");
                        archive.write(&entry, &pdf_bytes, false)?;
                        wrote += 1;
                    }
                    Err(error) => summary.failed.push((label.clone(), format!("{error:#}"))),
                }
            }
            // 整篇一个 PDF 都没导出时才报「没有盖章附件」；若编译件成功，
            // 该篇已部分导出，不误报为整篇失败。
            if missing_stamp && wrote == 0 {
                summary
                    .failed
                    .push((label, "没有盖章附件，已跳过盖章件".into()));
            }
            summary.pdfs += wrote;
            progress(&format!("已完成 {}/{} 篇", summary.records, total));
        }
        let file = archive.finish()?;
        file.sync_all()?;
        drop(file);
        temporary
            .persist(zip_path)
            .map_err(|error| error.error)
            .with_context(|| format!("无法写入导出文件 {}", zip_path.display()))?;
        Ok(summary)
    }
}

/// 在内存中编译单篇稿件，直接返回 PDF 字节。
/// 排版失败时视为该篇失败（原因写入汇总，不阻断整批）。
fn compile_record_pdf(
    record: &ManuscriptRecord,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
) -> Result<Vec<u8>> {
    compile_snapshot_pdf(
        &record.snapshot,
        &record.content_markdown,
        display,
        fonts,
        numbering,
    )
}

/// 按一份行文要素 + 正文在内存中排出非盖章件 PDF，返回字节。
/// 活稿行与历史版本快照（送批材料导出）都走这里，版式与单独导出一致。
pub(crate) fn compile_snapshot_pdf(
    snapshot: &crate::models::DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
) -> Result<Vec<u8>> {
    crate::export::compile_pdf(
        snapshot,
        markdown,
        display,
        fonts,
        numbering,
        &crate::visual_diff::ElementMarks::default(),
    )
    .map(|outcome| outcome.pdf)
}

/// 新格式必须完整携带活稿和历史版本引用的资源，缺一张图就拒绝导出。
fn collect_sync_images(records: &[ManifestRecord]) -> Result<(Vec<ManifestAsset>, Vec<Blob>)> {
    let base = crate::storage::config_dir()?;
    let mut paths = std::collections::BTreeSet::new();
    for record in records {
        for markdown in std::iter::once(&record.content_markdown).chain(
            record
                .revisions
                .iter()
                .map(|revision| &revision.content_markdown),
        ) {
            for src in crate::images::image_refs(markdown) {
                if src.starts_with("images/") {
                    paths.insert(src);
                }
            }
        }
    }
    let mut assets = Vec::new();
    let mut blobs = Vec::new();
    for path in paths {
        let source = crate::images::resolve_from_base(&base, &path)?;
        // 稿件包里的资源必须是明文：这里把本机密文解密后再算哈希、入包。
        let bytes = crate::images::read_managed_file(&source)
            .with_context(|| format!("稿件图片缺失：{}", source.display()))?;
        if bytes.len() as u64 > MAX_PDF_BYTES {
            bail!("稿件图片过大：{path}");
        }
        assets.push(ManifestAsset {
            path: path.clone(),
            sha256: bytes_hash(&bytes),
        });
        blobs.push((path, bytes));
    }
    Ok((assets, blobs))
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.schema == 1 {
        return Ok(());
    }
    let mut documents = std::collections::HashSet::new();
    let mut revisions = std::collections::HashSet::new();
    let assets = manifest
        .assets
        .iter()
        .map(|asset| (&asset.path, &asset.sha256))
        .collect::<std::collections::HashMap<_, _>>();
    if assets.len() != manifest.assets.len() {
        bail!("稿件包图片路径重复");
    }
    for asset in &manifest.assets {
        if !asset.path.starts_with("images/")
            || asset.path.contains('\\')
            || asset.path.split('/').count() != 2
            || asset
                .path
                .split('/')
                .any(|part| part == ".." || part == "." || part.is_empty())
            || asset.sha256.len() != 64
            || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("稿件包图片清单无效：{}", asset.path);
        }
    }
    for record in &manifest.records {
        let document_uuid = record
            .document_uuid
            .as_deref()
            .context("稿件包缺少稿件身份")?;
        let head = record
            .head_revision_uuid
            .as_deref()
            .context("稿件包缺少当前版本")?;
        if uuid::Uuid::parse_str(document_uuid).is_err() || !documents.insert(document_uuid) {
            bail!("稿件身份无效或重复：{document_uuid}");
        }
        let mut identity_set = std::collections::HashSet::new();
        identity_set.insert(document_uuid);
        for alias in &record.aliases {
            if uuid::Uuid::parse_str(alias).is_err()
                || !identity_set.insert(alias.as_str())
                || !documents.insert(alias.as_str())
            {
                bail!("稿件别名身份无效或重复：{alias}");
            }
        }
        let mut known = std::collections::HashSet::new();
        for revision in &record.revisions {
            revision.verify()?;
            if !revisions.insert(&revision.revision_uuid)
                || revision
                    .parents
                    .iter()
                    .any(|parent| !known.contains(parent.as_str()))
            {
                bail!("稿件版本重复、成环或祖先缺失：{}", revision.revision_uuid);
            }
            known.insert(revision.revision_uuid.as_str());
        }
        let head_revision = record
            .revisions
            .iter()
            .find(|revision| revision.revision_uuid == head)
            .context("稿件包缺少当前版本快照")?;
        if head_revision.snapshot != record.snapshot
            || head_revision.content_markdown != record.content_markdown
            || head_revision.notes != record.notes
        {
            bail!("稿件活稿与当前版本快照不一致：{}", record.title);
        }
        for markdown in std::iter::once(&record.content_markdown).chain(
            record
                .revisions
                .iter()
                .map(|revision| &revision.content_markdown),
        ) {
            for src in crate::images::image_refs(markdown) {
                if src.starts_with("images/") && !assets.contains_key(&src) {
                    bail!("稿件包缺少图片资源：{src}");
                }
            }
        }
        for pdf in &record.pdfs {
            if pdf.sha256.as_deref().is_none_or(|hash| hash.len() != 64) {
                bail!("稿件包 PDF 校验值缺失：{}", pdf.file_name);
            }
        }
    }
    Ok(())
}

/// 从清单记录中收集 markdown 引用的图片，返回 zip 条目（`images/<文件名>`）与字节。
/// 引用缺失或读取失败时跳过，不阻断导出。`base` 是配置目录（图片相对路径的基准）。
#[cfg(test)]
fn collect_image_entries(base: &Path, records: &[ManifestRecord]) -> Vec<(String, Vec<u8>)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for record in records {
        for src in crate::images::image_refs(&record.content_markdown) {
            if !seen.insert(src.clone()) {
                continue;
            }
            let Some(file_name) = src.rsplit('/').next() else {
                continue;
            };
            let Ok(source) = crate::images::resolve_from_base(base, &src) else {
                continue;
            };
            let Ok(bytes) = crate::images::read_managed_file(&source) else {
                continue;
            };
            out.push((format!("images/{}", sanitize_entry_name(file_name)), bytes));
        }
    }
    out
}

/// 按文件头识别 7z / 旧 ZIP 并解析清单，不落库（导入预览用）。
pub fn read_manifest(zip_path: &Path, password: &str) -> Result<Manifest> {
    if password.is_empty() {
        bail!("请输入稿件包密码");
    }
    let mut archive = PackageReader::open(zip_path, password)?;
    let raw = archive
        .read(MANIFEST_NAME, password, MANIFEST_MAX_BYTES)
        .context("无法读取 manifests.json")?;
    let manifest: Manifest = serde_json::from_slice(&raw).context("manifests.json 格式无效")?;
    if manifest.schema != 1 && manifest.schema != 2 && manifest.schema != MANIFEST_SCHEMA {
        bail!(
            "不支持的文件格式版本：v{}（当前支持 v1 / v{}）",
            manifest.schema,
            MANIFEST_SCHEMA
        );
    }
    validate_manifest(&manifest)?;
    Ok(manifest)
}

/// 只读稿件包中的标准词库。可选条目：旧包或未附带词库的包返回 `Ok(None)`，不阻断稿件导入。
/// 只有 `vocabulary.json` 缺失时视为无词库；文件损坏或版本不支持则报错。
pub fn read_vocabulary(zip_path: &Path, password: &str) -> Result<Option<VocabularyFile>> {
    if password.is_empty() {
        bail!("请输入稿件包密码");
    }
    let mut archive = PackageReader::open(zip_path, password)?;
    if !archive.contains(VOCABULARY_NAME) {
        return Ok(None);
    }
    let raw = archive.read(VOCABULARY_NAME, password, VOCABULARY_MAX_BYTES)?;
    let vocab: VocabularyFile = serde_json::from_slice(&raw).context("vocabulary.json 格式无效")?;
    if vocab.schema != 1 && vocab.schema != VOCABULARY_SCHEMA {
        bail!(
            "不支持的词库格式版本：v{}（当前支持 v{}）",
            vocab.schema,
            VOCABULARY_SCHEMA
        );
    }
    Ok(Some(vocab))
}

/// 按预览勾选导入。重新读取 zip 以保证与磁盘一致；记录 id 写入 source_id 列去重。
#[cfg(test)]
pub fn import_zip(
    store: &mut ManuscriptStore,
    zip_path: &Path,
    opts: &ImportOptions,
    password: &str,
) -> Result<ImportSummary> {
    let manifest = read_manifest(zip_path, password)?;
    if opts.selected.len() != manifest.records.len() {
        bail!("勾选状态与清单记录数不一致，请重新预览");
    }
    let mut summary = ImportSummary::default();
    let mut archive = PackageReader::open(zip_path, password)?;
    // 恢复图片资源：图片是跨稿件共享目录，全量解压；旧包无 images/ 条目时无操作。
    if let Ok(image_dir) = crate::images::image_dir() {
        restore_images(&mut archive, &image_dir, password)?;
    }
    let existing = store.source_ids()?;
    for (index, record) in manifest.records.iter().enumerate() {
        if !opts.selected.get(index).copied().unwrap_or(false) {
            continue;
        }
        if opts.skip_existing_by_id && existing.contains(&record.id) {
            summary.skipped_existing += 1;
            continue;
        }
        let new_id = store.create(
            &NewManuscript {
                snapshot: record.snapshot.clone(),
                content_markdown: record.content_markdown.clone(),
                notes: record.notes.clone(),
                status: record.status,
                created_at: Some(record.created_at.clone()),
                updated_at: Some(record.updated_at.clone()),
                published_at: record.published_at.clone(),
                archived_at: record.archived_at.clone(),
            },
            Some(record.id),
        )?;
        summary.imported += 1;
        for pdf in &record.pdfs {
            let Ok(bytes) = archive.read(&pdf.path, password, MAX_PDF_BYTES) else {
                summary.skipped_pdfs += 1;
                continue;
            };
            store.add_pdf(new_id, &pdf.file_name, &bytes)?;
            summary.pdfs_imported += 1;
        }
    }
    Ok(summary)
}

/// 净化 zip 条目名：去掉路径分隔符、控制字符与 Windows 非法字符。
fn sanitize_entry_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if out.trim().is_empty() {
        out = "pdf.pdf".into();
    }
    out
}

/// 为 zip 内条目名去重：不同稿件可能生成相同主干（同名同文号），zip 不允许
/// 重名条目，同名时在扩展名前追加 `(2)`、`(3)`…，与目录导出的 `-2` 编号区分。
/// `used` 记录已占用的条目名，返回唯一可用的 `{stem}.{ext}`。
fn unique_zip_name(used: &mut std::collections::HashSet<String>, stem: &str, ext: &str) -> String {
    let base = format!("{stem}.{ext}");
    if used.insert(base.clone()) {
        return base;
    }
    let mut index = 2u64;
    loop {
        let candidate = format!("{stem}({index}).{ext}");
        index += 1;
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
}

/// 从 zip 恢复 `images/` 条目到目标目录。条目名经过净化，防止篡改的 zip
/// 用路径穿越覆盖任意文件；返回恢复的文件数。
#[cfg(test)]
fn restore_images(archive: &mut PackageReader, target: &Path, password: &str) -> Result<usize> {
    std::fs::create_dir_all(target)
        .with_context(|| format!("无法创建图片目录 {}", target.display()))?;
    let names: Vec<String> = archive
        .file_names()
        .into_iter()
        .filter(|name| name.starts_with("images/"))
        .collect();
    let mut count = 0usize;
    // 单张图片与整包图片都有上限：被篡改的 zip 不能靠海量条目吃光内存。
    let mut total: u64 = 0;
    for name in names {
        let file_name = name.strip_prefix("images/").unwrap_or(&name);
        let safe = sanitize_entry_name(file_name);
        let bytes = archive.read(&name, password, MAX_IMAGE_BYTES)?;
        total = total.saturating_add(bytes.len() as u64);
        anyhow::ensure!(
            total <= MAX_IMAGES_TOTAL_BYTES,
            "zip 里的图片合计超过 {} MB",
            MAX_IMAGES_TOTAL_BYTES / 1024 / 1024
        );
        let dest = target.join(&safe);
        // 目标已存在且内容一致时跳过，避免无谓写入；内容不同则按恢复语义覆盖。
        if crate::images::read_managed_file(&dest).ok().as_deref() == Some(bytes.as_slice()) {
            count += 1;
            continue;
        }
        crate::images::write_image_file(&dest, &bytes)
            .with_context(|| format!("无法恢复图片 {}", dest.display()))?;
        count += 1;
    }
    Ok(count)
}

fn map_zip_password_error(error: zip::result::ZipError) -> zip::result::ZipError {
    use zip::result::ZipError;
    match error {
        ZipError::InvalidPassword => ZipError::InvalidArchive("ZIP 密码错误".into()),
        ZipError::UnsupportedArchive(message) if message == ZipError::PASSWORD_REQUIRED => {
            ZipError::InvalidArchive("ZIP 密码错误或文件未正确加密".into())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::{ManuscriptFilter, ManuscriptStore, ManuscriptUpdate};
    use crate::models::{TemplateProfile, VocabularyCategory};

    const TEST_PASSWORD: &str = "Jade!River7Cloud";

    fn sample_snapshot(kind: TemplateKind) -> DraftInput {
        DraftInput {
            kind,
            title_hint: "关于报送情况的通知".into(),
            date: "2026年8月6日".into(),
            date_is_auto: false,
            meeting_time: String::new(),
            attendees: String::new(),
            profile: TemplateProfile {
                document_number: "某教函〔2026〕12号".into(),
                ..TemplateProfile::for_kind(kind)
            },
            research: Default::default(),
            phone_record: Default::default(),
            minutes: Default::default(),
        }
    }

    fn mem_store() -> ManuscriptStore {
        ManuscriptStore::open(Path::new(":memory:")).unwrap()
    }

    fn sample_vocabulary() -> Vec<VocabularyEntry> {
        vec![
            VocabularyEntry {
                id: 1,
                category: VocabularyCategory::Unit,
                code: "00".into(),
                canonical: "某省教育厅".into(),
                department_code: "某教".into(),
                ..Default::default()
            },
            VocabularyEntry {
                id: 2,
                category: VocabularyCategory::Person,
                canonical: "张三".into(),
                unit: "00".into(),
                position: "处长".into(),
                phone: "13800000000".into(),
                ..Default::default()
            },
        ]
    }

    fn seed(store: &mut ManuscriptStore) -> i64 {
        let id = store
            .create(
                &NewManuscript {
                    snapshot: sample_snapshot(TemplateKind::OfficialLetter),
                    content_markdown: "# 关于报送情况的通知\n\n正文".into(),
                    notes: String::new(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        store.add_pdf(id, "扫描件.pdf", b"%PDF-1.4 first").unwrap();
        store.add_pdf(id, "盖章件.pdf", b"%PDF-1.4 second").unwrap();
        store.set_status(id, ManuscriptStatus::Published).unwrap();
        store.set_status(id, ManuscriptStatus::Archived).unwrap();
        id
    }

    /// 导出命名主干：与实现同源，但用于断言组装逻辑（盖章后缀、序号、去重、无时间戳）。
    fn expected_stem(record: &ManuscriptRecord) -> String {
        crate::export::document_stem_prefix(
            &record.snapshot,
            &crate::export::document_title(&record.snapshot, &record.content_markdown),
        )
    }

    fn zip_names(zip_path: &Path) -> Vec<String> {
        let mut names = PackageReader::open(zip_path, TEST_PASSWORD)
            .unwrap()
            .file_names();
        names.sort();
        names
    }

    fn read_zip_entry(zip_path: &Path, name: &str) -> String {
        let mut archive = PackageReader::open(zip_path, TEST_PASSWORD).unwrap();
        String::from_utf8(archive.read(name, TEST_PASSWORD, MAX_PDF_BYTES).unwrap()).unwrap()
    }

    #[test]
    fn export_selected_pdfs_stamped_only_naming() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("pdf.zip");
        let mut store = mem_store();
        let id = seed(&mut store);
        let options = PdfExportOptions {
            stamped: true,
            compiled: false,
        };
        let summary = export_selected_pdfs(
            &mut store,
            &[id],
            &options,
            &sample_vocabulary(),
            &FontConfig::default(),
            &NumberingConfig::default(),
            &zip_path,
            TEST_PASSWORD,
            |_| {},
        )
        .unwrap();
        assert_eq!(summary.records, 1);
        assert_eq!(summary.pdfs, 2);
        assert!(summary.failed.is_empty());

        let stem = expected_stem(&store.get(id).unwrap().unwrap());
        // 盖章件命名：主干 +（盖章）；多个附件依次（盖章2）…；不含分钟级时间戳。
        let names = zip_names(&zip_path);
        assert_eq!(names.len(), 2);
        let first = format!("{stem}（盖章）.pdf");
        let second = format!("{stem}（盖章2）.pdf");
        assert!(
            names.contains(&first) && names.contains(&second),
            "盖章件命名应为 {first} 与 {second}，实际：{names:?}"
        );
        let timestamp = chrono::Local::now().format("%Y%m%d%H%M").to_string();
        for name in &names {
            assert!(
                !name.contains(&format!("-{timestamp}")),
                "导出文件名不应带时间戳：{name}"
            );
        }
        // 附件字节原样写入，按添加顺序对应。
        assert_eq!(
            read_zip_entry(&zip_path, &format!("{stem}（盖章）.pdf")),
            "%PDF-1.4 first"
        );
        assert_eq!(
            read_zip_entry(&zip_path, &format!("{stem}（盖章2）.pdf")),
            "%PDF-1.4 second"
        );
    }

    #[test]
    fn export_selected_pdfs_duplicate_stems_are_numbered() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("dup.zip");
        let mut store = mem_store();
        let id1 = seed(&mut store);
        // 第二篇与第一篇同标题、同文号，导出主干相同，条目名必须去重。
        let id2 = store
            .create(
                &NewManuscript {
                    snapshot: sample_snapshot(TemplateKind::OfficialLetter),
                    content_markdown: "# 关于报送情况的通知\n\n另一篇正文".into(),
                    notes: String::new(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        store.add_pdf(id2, "盖章2.pdf", b"%PDF-1.4 dup").unwrap();
        let options = PdfExportOptions {
            stamped: true,
            compiled: false,
        };
        let summary = export_selected_pdfs(
            &mut store,
            &[id1, id2],
            &options,
            &sample_vocabulary(),
            &FontConfig::default(),
            &NumberingConfig::default(),
            &zip_path,
            TEST_PASSWORD,
            |_| {},
        )
        .unwrap();
        assert_eq!(summary.records, 2);
        assert_eq!(summary.pdfs, 3);

        let names = zip_names(&zip_path);
        assert_eq!(names.len(), 3);
        let unique = names.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), 3, "zip 条目名必须互不相同：{names:?}");
        assert!(
            names.iter().any(|name| name.contains("(2)")),
            "重名条目应追加 (2) 编号：{names:?}"
        );
    }

    #[test]
    fn export_selected_pdfs_no_options_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("none.zip");
        let mut store = mem_store();
        let id = seed(&mut store);
        let options = PdfExportOptions {
            stamped: false,
            compiled: false,
        };
        let error = export_selected_pdfs(
            &mut store,
            &[id],
            &options,
            &sample_vocabulary(),
            &FontConfig::default(),
            &NumberingConfig::default(),
            &zip_path,
            TEST_PASSWORD,
            |_| {},
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("至少选择"), "{error:#}");
    }

    #[test]
    fn export_selected_pdfs_typesets_compiled_pdfs() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("compiled.zip");
        let mut store = mem_store();
        let id = seed(&mut store);
        let options = PdfExportOptions {
            stamped: false,
            compiled: true,
        };
        let summary = export_selected_pdfs(
            &mut store,
            &[id],
            &options,
            &sample_vocabulary(),
            &FontConfig::default(),
            &NumberingConfig::default(),
            &zip_path,
            TEST_PASSWORD,
            |_| {},
        )
        .unwrap();
        assert_eq!(summary.records, 1);
        // 排版在进程内完成；只有找不到随包字体（沙箱环境）时才记入失败，不应 panic。
        assert!(
            summary.pdfs == 1 || !summary.failed.is_empty(),
            "应产出 PDF 或记入失败：{summary:?}"
        );
        let names = zip_names(&zip_path);
        assert!(names.len() <= 1);
    }

    #[test]
    fn export_import_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("稿件.zip");

        let mut source = mem_store();
        let id = seed(&mut source);
        let summary = export_zip(
            &mut source,
            &ManuscriptFilter::default(),
            &[],
            &zip_path,
            TEST_PASSWORD,
            false,
        )
        .unwrap();
        assert_eq!(summary.records, 1);
        assert_eq!(summary.pdfs, 2);

        // 导出后源库记录与清单记录 id 一致。
        let manifest = read_manifest(&zip_path, TEST_PASSWORD).unwrap();
        assert_eq!(manifest.schema, MANIFEST_SCHEMA);
        assert_eq!(manifest.records.len(), 1);
        assert_eq!(manifest.records[0].id, id);
        assert_eq!(manifest.records[0].status, ManuscriptStatus::Archived);
        assert!(manifest.records[0].archived_at.is_some());

        // 导入到空库：记录与附件全部还原，归档态保留。
        let mut dest = mem_store();
        let opts = ImportOptions {
            skip_existing_by_id: true,
            selected: vec![true],
        };
        let result = import_zip(&mut dest, &zip_path, &opts, TEST_PASSWORD).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.pdfs_imported, 2);
        assert_eq!(result.skipped_existing, 0);

        let imported = dest.list(&ManuscriptFilter::default()).unwrap();
        assert_eq!(imported.len(), 1);
        let record = dest.get(imported[0].id).unwrap().unwrap();
        assert_eq!(record.status, ManuscriptStatus::Archived);
        assert_eq!(record.archived_at, manifest.records[0].archived_at);
        assert_eq!(record.source_id, Some(id));
        assert_eq!(record.pdfs.len(), 2);
        assert_eq!(record.pdfs[0].file_name, "扫描件.pdf");
        assert_eq!(record.pdfs[0].bytes, b"%PDF-1.4 first");
        assert_eq!(record.pdfs[1].bytes, b"%PDF-1.4 second");
        // 归档行在导入后依然不可改。
        assert!(
            dest.update(
                imported[0].id,
                &ManuscriptUpdate {
                    snapshot: sample_snapshot(TemplateKind::OfficialLetter),
                    content_markdown: "改".into(),
                    notes: String::new(),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn prepared_export_freezes_body_and_failed_write_preserves_existing_package() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = mem_store();
        let snapshot = sample_snapshot(TemplateKind::OfficialLetter);
        let id = source
            .create(
                &NewManuscript {
                    snapshot: snapshot.clone(),
                    content_markdown: "# 本次导出\n\n导出时的正文。".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let prepared = prepare_package(&mut source, &[id], &[]).unwrap();
        source
            .update(
                id,
                &ManuscriptUpdate {
                    snapshot,
                    content_markdown: "# 后续修改\n\n后台打包期间的修改。".into(),
                    notes: String::new(),
                },
            )
            .unwrap();
        let path = dir.path().join("frozen.gwx");
        prepared.write(&path, TEST_PASSWORD).unwrap();
        assert!(
            read_manifest(&path, TEST_PASSWORD).unwrap().records[0]
                .content_markdown
                .contains("导出时的正文")
        );
        let original = std::fs::read(&path).unwrap();
        let invalid = prepare_package(&mut source, &[id], &[]).unwrap();
        assert!(invalid.write(&path, "weak").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "失败不留下明文或半成品临时文件"
        );
    }

    #[test]
    fn exported_zip_requires_the_correct_password() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("加密稿件.zip");
        let mut store = mem_store();
        seed(&mut store);
        export_zip(
            &mut store,
            &ManuscriptFilter::default(),
            &sample_vocabulary(),
            &zip_path,
            TEST_PASSWORD,
            false,
        )
        .unwrap();

        let raw = std::fs::read(&zip_path).unwrap();
        assert!(raw.starts_with(b"7z\xbc\xaf\x27\x1c"));
        assert!(
            sevenz_rust2::ArchiveReader::open(&zip_path, sevenz_rust2::Password::empty()).is_err(),
            "无密码时连文件目录也不能读取"
        );
        let archive = sevenz_rust2::ArchiveReader::open(&zip_path, TEST_PASSWORD.into()).unwrap();
        assert!(
            archive
                .archive()
                .files
                .iter()
                .enumerate()
                .all(|(i, _)| archive.archive().is_file_encrypted(i)),
            "每个非空条目都须加密"
        );
        drop(archive);

        let error = read_manifest(&zip_path, "Wrong!Key8Stone").unwrap_err();
        assert!(format!("{error:#}").contains("密码错误"), "{error:#}");
        assert!(read_manifest(&zip_path, TEST_PASSWORD).is_ok());
    }

    #[test]
    fn legacy_zip_schemas_and_weak_password_still_import_with_pdfs_and_vocabulary() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = mem_store();
        crate::storage::set_test_config_dir(Some(dir.path().join("resources")));
        let image_path = crate::images::image_dir().unwrap().join("legacy.png");
        crate::images::write_image_file(&image_path, b"legacy-image-bytes").unwrap();
        let id = source
            .create(
                &NewManuscript {
                    snapshot: sample_snapshot(TemplateKind::OfficialLetter),
                    content_markdown: "# 旧稿件\n\n![旧图](images/legacy.png)\n正文。".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        source.add_pdf(id, "扫描件.pdf", b"%PDF-1.4 first").unwrap();
        source
            .add_pdf(id, "盖章件.pdf", b"%PDF-1.4 second")
            .unwrap();
        let new_path = dir.path().join("new.gwx");
        export_zip(
            &mut source,
            &Default::default(),
            &sample_vocabulary(),
            &new_path,
            TEST_PASSWORD,
            false,
        )
        .unwrap();
        let mut new_archive = PackageReader::open(&new_path, TEST_PASSWORD).unwrap();
        // 用旧版同一套 ZIP AES 写法构造 v1/v2/v3，旧弱口令不受新强度规则阻挡。
        for schema in [1, 2, 3] {
            for extension in ["zip", "gwx"] {
                let path = dir.path().join(format!("legacy-v{schema}.{extension}"));
                let mut writer = ZipWriter::new(File::create(&path).unwrap());
                for name in new_archive.file_names() {
                    let mut bytes = new_archive
                        .read(&name, TEST_PASSWORD, MAX_PDF_BYTES)
                        .unwrap();
                    if name == MANIFEST_NAME {
                        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        value["schema"] = schema.into();
                        if schema == 1 {
                            for record in value["records"].as_array_mut().unwrap() {
                                for field in [
                                    "document_uuid",
                                    "aliases",
                                    "head_revision_uuid",
                                    "revisions",
                                ] {
                                    record.as_object_mut().unwrap().remove(field);
                                }
                            }
                        }
                        bytes = serde_json::to_vec(&value).unwrap();
                    }
                    writer.start_file(name, encrypted_options("1234")).unwrap();
                    writer.write_all(&bytes).unwrap();
                }
                writer.finish().unwrap();
                assert!(read_manifest(&path, "wrong").is_err());
                let manifest = read_manifest(&path, "1234").unwrap();
                assert_eq!(manifest.schema, schema);
                assert_eq!(
                    read_vocabulary(&path, "1234")
                        .unwrap()
                        .unwrap()
                        .entries
                        .len(),
                    sample_vocabulary().len()
                );
                // 使用生产的身份判定与事务导入通路，验证不是只能读到预览。
                std::fs::remove_file(&image_path).unwrap();
                let mut target = mem_store();
                let previews = sync::inspect(&mut target, &manifest).unwrap();
                let hash = bytes_hash(&serde_json::to_vec(&manifest).unwrap());
                let actions = vec![sync::ImportAction::New];
                let result = sync::import_with_actions(
                    &mut target,
                    &path,
                    "1234",
                    &hash,
                    &actions,
                    &previews,
                )
                .unwrap();
                assert_eq!(result.pdfs_added, 2);
                assert_eq!(
                    crate::images::read_managed_file(&image_path).unwrap(),
                    b"legacy-image-bytes"
                );
                let rows = target.list(&Default::default()).unwrap();
                assert_eq!(rows.len(), 1);
                let imported = target.get(rows[0].id).unwrap().unwrap();
                assert_eq!(
                    imported.content_markdown,
                    manifest.records[0].content_markdown
                );
                assert_eq!(imported.pdfs[0].bytes, b"%PDF-1.4 first");
                assert_eq!(imported.pdfs[1].bytes, b"%PDF-1.4 second");
                if schema >= 2 {
                    assert_eq!(
                        target.document_identity(rows[0].id).unwrap().0,
                        manifest.records[0].document_uuid.as_ref().unwrap().as_str()
                    );
                }
            }
        }
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn export_password_strength_is_enforced() {
        assert!(validate_export_password(TEST_PASSWORD).is_ok());
        for weak in [
            "Short!7A",
            "alllowercase7",
            "Password!7Cloud",
            "Safe!1234Cloud",
            "Safe!7777Cloud",
            " Safe!River7Cloud",
        ] {
            assert!(
                validate_export_password(weak).is_err(),
                "弱密码应被拒绝：{weak}"
            );
        }
        assert!(validate_export_password("安全!公文7云端备份").is_ok());
    }

    #[test]
    fn vocabulary_export_import_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("稿件.zip");
        let mut store = mem_store();
        seed(&mut store);
        export_zip(
            &mut store,
            &ManuscriptFilter::default(),
            &sample_vocabulary(),
            &zip_path,
            TEST_PASSWORD,
            false,
        )
        .unwrap();

        let read = read_vocabulary(&zip_path, TEST_PASSWORD)
            .unwrap()
            .expect("包内应带词库");
        assert_eq!(read.schema, VOCABULARY_SCHEMA);
        assert_eq!(read.entries.len(), 2);
        let unit = read
            .entries
            .iter()
            .find(|entry| entry.category == VocabularyCategory::Unit)
            .unwrap();
        assert_eq!(unit.code, "00");
        assert_eq!(unit.canonical, "某省教育厅");
        assert_eq!(unit.department_code, "某教");
        let person = read
            .entries
            .iter()
            .find(|entry| entry.category == VocabularyCategory::Person)
            .unwrap();
        assert_eq!(person.unit, "00");
        assert_eq!(person.phone, "13800000000");
        assert_eq!(person.position, "处长");
    }

    #[test]
    fn vocabulary_omitted_when_empty() {
        // 空词库不写 vocabulary.json：读取视为无词库，与旧包（只有 manifests.json）行为一致。
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("稿件.zip");
        let mut store = mem_store();
        seed(&mut store);
        export_zip(
            &mut store,
            &ManuscriptFilter::default(),
            &[],
            &zip_path,
            TEST_PASSWORD,
            false,
        )
        .unwrap();
        assert!(read_vocabulary(&zip_path, TEST_PASSWORD).unwrap().is_none());
    }

    #[test]
    fn vocabulary_unsupported_schema_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("旧版词库.zip");
        let file = File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file(VOCABULARY_NAME, SimpleFileOptions::default())
            .unwrap();
        let fake = serde_json::json!({
            "schema": 99,
            "exported_at": "2026-01-01T00:00:00+08:00",
            "entries": []
        });
        zip.write_all(serde_json::to_string_pretty(&fake).unwrap().as_bytes())
            .unwrap();
        zip.finish().unwrap();
        assert!(read_vocabulary(&zip_path, TEST_PASSWORD).is_err());
    }

    #[test]
    fn vocabulary_oversized_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("超大词库.zip");
        let file = File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file(VOCABULARY_NAME, SimpleFileOptions::default())
            .unwrap();
        let blob = vec![b'x'; (VOCABULARY_MAX_BYTES + 1) as usize];
        zip.write_all(&blob).unwrap();
        zip.finish().unwrap();
        let error = read_vocabulary(&zip_path, TEST_PASSWORD).unwrap_err();
        assert!(format!("{error:#}").contains("过大"));
    }

    #[test]
    fn collect_image_entries_deduplicates_and_skips_missing() {
        let base = tempfile::tempdir().unwrap();
        let img_dir = base.path().join("images");
        std::fs::create_dir_all(&img_dir).unwrap();
        std::fs::write(img_dir.join("a.png"), b"png-a").unwrap();
        std::fs::write(img_dir.join("b.pdf"), b"%PDF").unwrap();
        let record = ManifestRecord {
            id: 1,
            title: "标题".into(),
            kind: TemplateKind::OfficialLetter,
            status: ManuscriptStatus::Draft,
            doc_number: String::new(),
            doc_date: String::new(),
            notes: String::new(),
            content_markdown: "![a](images/a.png)\n![b](images/b.pdf)\n![缺](images/missing.png)\n![a](images/a.png)"
                .into(),
            snapshot: sample_snapshot(TemplateKind::OfficialLetter),
            created_at: String::new(),
            updated_at: String::new(),
            published_at: None,
            archived_at: None,
            pdfs: Vec::new(),
            document_uuid: None,
            aliases: Vec::new(),
            head_revision_uuid: None,
            revisions: Vec::new(),
            send_package: None,
        };
        let entries = collect_image_entries(base.path(), &[record]);
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|(p, b)| p == "images/a.png" && b == b"png-a")
        );
        assert!(
            entries
                .iter()
                .any(|(p, b)| p == "images/b.pdf" && b == b"%PDF")
        );
    }

    #[test]
    fn restore_images_extracts_and_sanitizes_entries() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("img.zip");
        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = ZipWriter::new(file);
            zip.start_file("images/a.png", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"png-a").unwrap();
            // 恶意条目名带路径穿越，恢复时必须净化，不能写到目标目录之外。
            zip.start_file("images/../evil.png", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"evil").unwrap();
            zip.finish().unwrap();
        }
        let target = tempfile::tempdir().unwrap();
        let mut archive = PackageReader::open(&zip_path, TEST_PASSWORD).unwrap();
        let count = restore_images(&mut archive, target.path(), TEST_PASSWORD).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            std::fs::read(target.path().join("a.png")).unwrap(),
            b"png-a"
        );
        assert_eq!(
            std::fs::read(target.path().join(".._evil.png")).unwrap(),
            b"evil"
        );
        assert!(!target.path().join("..").join("evil.png").exists());
        // 再次恢复同一包：内容一致时跳过，不产生新写入，计数不变。
        let mut archive = PackageReader::open(&zip_path, TEST_PASSWORD).unwrap();
        assert_eq!(
            restore_images(&mut archive, target.path(), TEST_PASSWORD).unwrap(),
            2
        );
        assert_eq!(
            std::fs::read(target.path().join("a.png")).unwrap(),
            b"png-a"
        );
    }

    #[test]
    fn reimport_skips_existing_by_source_id() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("稿件.zip");
        let mut source = mem_store();
        seed(&mut source);
        export_zip(
            &mut source,
            &ManuscriptFilter::default(),
            &[],
            &zip_path,
            TEST_PASSWORD,
            false,
        )
        .unwrap();

        let mut dest = mem_store();
        let opts = ImportOptions {
            skip_existing_by_id: true,
            selected: vec![true],
        };
        import_zip(&mut dest, &zip_path, &opts, TEST_PASSWORD).unwrap();
        let second = import_zip(&mut dest, &zip_path, &opts, TEST_PASSWORD).unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped_existing, 1);
        assert_eq!(dest.list(&ManuscriptFilter::default()).unwrap().len(), 1);
    }

    #[test]
    fn import_respects_selection() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("稿件.zip");
        let mut source = mem_store();
        seed(&mut source);
        export_zip(
            &mut source,
            &ManuscriptFilter::default(),
            &[],
            &zip_path,
            TEST_PASSWORD,
            false,
        )
        .unwrap();

        let mut dest = mem_store();
        let opts = ImportOptions {
            skip_existing_by_id: true,
            selected: vec![false],
        };
        let result = import_zip(&mut dest, &zip_path, &opts, TEST_PASSWORD).unwrap();
        assert_eq!(result.imported, 0);
        assert!(dest.list(&ManuscriptFilter::default()).unwrap().is_empty());
    }

    #[test]
    fn export_selected_includes_only_requested_records() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("所选.zip");
        let mut store = mem_store();
        let first = store
            .create(
                &NewManuscript {
                    snapshot: sample_snapshot(TemplateKind::OfficialLetter),
                    content_markdown: "# 第一篇".into(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let second = store
            .create(
                &NewManuscript {
                    snapshot: sample_snapshot(TemplateKind::OfficialLetter),
                    content_markdown: "# 第二篇".into(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();

        let summary =
            export_zip_selected(&mut store, &[second], &[], &zip_path, TEST_PASSWORD, false)
                .unwrap();
        assert_eq!(summary.records, 1);
        let manifest = read_manifest(&zip_path, TEST_PASSWORD).unwrap();
        assert_eq!(manifest.records.len(), 1);
        assert_eq!(manifest.records[0].id, second);
        assert_ne!(manifest.records[0].id, first);
    }

    #[test]
    fn export_with_empty_filter_bails() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("空.zip");
        let mut store = mem_store();
        store
            .create(
                &NewManuscript {
                    snapshot: sample_snapshot(TemplateKind::OfficialLetter),
                    content_markdown: "# 甲".into(),
                    notes: String::new(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        // 过滤条件命中不到任何记录 → 报错，不产生文件。
        let filter = ManuscriptFilter {
            kind: Some(TemplateKind::MeetingAgenda),
            ..Default::default()
        };
        assert!(export_zip(&mut store, &filter, &[], &zip_path, TEST_PASSWORD, false).is_err());
        assert!(!zip_path.exists());
    }

    #[test]
    fn unsupported_schema_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("旧版.zip");
        let file = File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file(MANIFEST_NAME, SimpleFileOptions::default())
            .unwrap();
        let fake = serde_json::json!({
            "schema": 99,
            "exported_at": "2026-01-01T00:00:00+08:00",
            "records": []
        });
        zip.write_all(serde_json::to_string_pretty(&fake).unwrap().as_bytes())
            .unwrap();
        zip.finish().unwrap();
        assert!(read_manifest(&zip_path, TEST_PASSWORD).is_err());
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize_entry_name("扫描件.pdf"), "扫描件.pdf");
        assert_eq!(sanitize_entry_name("a/b\\c:d*e?.pdf"), "a_b_c_d_e_.pdf");
        assert_eq!(sanitize_entry_name(""), "pdf.pdf");
    }
}
