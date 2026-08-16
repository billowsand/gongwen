//! 稿件 ZIP 导出/导入。
//!
//! ZIP 布局：`manifests.json`（带 schema 版本）+ `pdf/<id>_<序号>_<净化文件名>` 附件。
//! 导出按 `ManuscriptFilter` 过滤；导入由预览勾选 + 关键词过滤 + `skip_existing_by_id`
//! 决定写哪些记录，满足“导入也支持过滤筛选”。

use crate::manuscript::{ManuscriptFilter, ManuscriptRecord, ManuscriptStore, NewManuscript};
use crate::models::{DraftInput, FontConfig, ManuscriptStatus, TemplateKind, VocabularyEntry};
use crate::units::UnitDisplay;
use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use zip::read::ZipArchive;
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipWriter};

pub const MANIFEST_SCHEMA: u32 = 1;
const MANIFEST_NAME: &str = "manifests.json";
/// 随稿件包导出的标准词库（全局一份，随包带走，导入时增量合并）。
pub const VOCABULARY_SCHEMA: u32 = 1;
const VOCABULARY_NAME: &str = "vocabulary.json";
/// 词库 JSON 体积上限（约 10 MB），远超正常词库规模，防恶意包撑爆内存。
const VOCABULARY_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 单个 PDF 附件上限（约 100 MB），超限跳过，避免导入超大文件撑爆库。
const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;

/// 导出密码规则：不少于 10 个字符，且至少覆盖三类字符；同时拒绝常见口令、
/// 连续字符和长重复字符。导入不调用此校验，以兼容外部工具生成的历史弱密码包。
pub fn validate_export_password(password: &str) -> Result<()> {
    if password.trim() != password {
        bail!("密码首尾不能包含空格");
    }
    let chars = password.chars().collect::<Vec<_>>();
    if chars.len() < 10 {
        bail!("密码至少需要 10 个字符");
    }
    if chars.len() > 128 {
        bail!("密码不能超过 128 个字符");
    }

    let categories = [
        chars.iter().any(|ch| ch.is_ascii_lowercase()),
        chars.iter().any(|ch| ch.is_ascii_uppercase()),
        chars.iter().any(|ch| ch.is_numeric()),
        chars.iter().any(|ch| !ch.is_alphanumeric()),
        chars.iter().any(|ch| ch.is_alphabetic() && !ch.is_ascii()),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if categories < 3 {
        bail!("密码需包含大写字母、小写字母、数字、符号或中文中的至少三类");
    }

    let lower = password.to_lowercase();
    const COMMON: &[&str] = &[
        "password",
        "qwerty",
        "admin",
        "letmein",
        "iloveyou",
        "123456",
        "111111",
        "abcdef",
        "公文助手",
    ];
    if COMMON.iter().any(|word| lower.contains(word)) {
        bail!("密码包含常见口令或简单序列，请换一个更复杂的密码");
    }
    if has_ascii_sequence(&lower) {
        bail!("密码不能包含 4 位及以上连续字母或数字");
    }
    if chars
        .windows(4)
        .any(|window| window.iter().all(|ch| *ch == window[0]))
    {
        bail!("密码不能包含 4 个及以上连续相同字符");
    }
    Ok(())
}

fn has_ascii_sequence(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(4).any(|window| {
        window.iter().all(u8::is_ascii_alphanumeric)
            && (window.windows(2).all(|pair| pair[1] == pair[0] + 1)
                || window.windows(2).all(|pair| pair[0] == pair[1] + 1))
    })
}

fn encrypted_options(password: &str) -> zip::write::FileOptions<'_, ()> {
    SimpleFileOptions::default().with_aes_encryption(AesMode::Aes256, password)
}

fn encrypted_stored_options(password: &str) -> zip::write::FileOptions<'_, ()> {
    encrypted_options(password).compression_method(CompressionMethod::Stored)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPdf {
    pub id: i64,
    pub file_name: String,
    /// zip 内相对路径，如 `pdf/3_0_扫描件.pdf`。
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestRecord {
    /// 导出用源库 id（`source_id.unwrap_or(local_id)`）；导入写入 source_id 列做去重。
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
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub exported_at: String,
    pub records: Vec<ManifestRecord>,
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

#[derive(Debug, Default)]
pub struct ImportSummary {
    pub imported: usize,
    pub skipped_existing: usize,
    pub pdfs_imported: usize,
    /// 附件在 zip 中缺失或超出大小上限，跳过但不中断整批导入。
    pub skipped_pdfs: usize,
}

#[derive(Debug)]
pub struct ImportOptions {
    pub skip_existing_by_id: bool,
    /// 与 manifest.records 等长；true 才导入。
    pub selected: Vec<bool>,
}

/// 按过滤条件导出稿件（含 PDF 附件）为 ZIP。`vocabulary` 为标准词库，非空时随包导出，
/// 便于把稿件带到另一台电脑后保持要素一致。没有符合条件稿件时直接报错。
pub fn export_zip(
    store: &mut ManuscriptStore,
    filter: &ManuscriptFilter,
    vocabulary: &[VocabularyEntry],
    zip_path: &Path,
    password: &str,
) -> Result<ExportSummary> {
    validate_export_password(password)?;
    let rows = store.list(filter)?;
    if rows.is_empty() {
        bail!("没有符合过滤条件的稿件");
    }
    let ids = rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
    export_zip_ids(store, &ids, vocabulary, zip_path, password)
}

/// 只导出稿件管理页明确勾选的记录。
pub fn export_zip_selected(
    store: &mut ManuscriptStore,
    ids: &[i64],
    vocabulary: &[VocabularyEntry],
    zip_path: &Path,
    password: &str,
) -> Result<ExportSummary> {
    validate_export_password(password)?;
    export_zip_ids(store, ids, vocabulary, zip_path, password)
}

fn export_zip_ids(
    store: &mut ManuscriptStore,
    ids: &[i64],
    vocabulary: &[VocabularyEntry],
    zip_path: &Path,
    password: &str,
) -> Result<ExportSummary> {
    if ids.is_empty() {
        bail!("没有选中要导出的稿件");
    }
    let mut records = Vec::new();
    let mut pdf_blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total_pdfs = 0usize;
    for &id in ids {
        let Some(record) = store.get(id)? else {
            continue;
        };
        let export_id = record.source_id.unwrap_or(record.id);
        let mut pdfs = Vec::new();
        for (idx, pdf) in record.pdfs.iter().enumerate() {
            let entry = format!(
                "pdf/{export_id}_{idx}_{}",
                sanitize_entry_name(&pdf.file_name)
            );
            pdf_blobs.push((entry.clone(), pdf.bytes.clone()));
            pdfs.push(ManifestPdf {
                id: pdf.id,
                file_name: pdf.file_name.clone(),
                path: entry,
            });
            total_pdfs += 1;
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
        });
    }
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA,
        exported_at: Local::now().to_rfc3339(),
        records,
    };
    // 收集全部稿件引用的图片（跨稿件去重），zip 条目平铺为 images/<文件名>。
    let image_blobs = match crate::storage::config_dir() {
        Ok(base) => collect_image_entries(&base, &manifest.records),
        Err(_) => Vec::new(),
    };

    let file = File::create(zip_path)
        .with_context(|| format!("无法创建导出文件 {}", zip_path.display()))?;
    let mut zip = ZipWriter::new(file);
    zip.start_file(MANIFEST_NAME, encrypted_options(password))?;
    zip.write_all(
        serde_json::to_string_pretty(&manifest)
            .context("序列化清单失败")?
            .as_bytes(),
    )?;
    // 词库非空才随包：目标机器导入时可选择增量合并，保证单位、人员、联系方式一致。
    if !vocabulary.is_empty() {
        let vocab_file = VocabularyFile {
            schema: VOCABULARY_SCHEMA,
            exported_at: Local::now().to_rfc3339(),
            entries: vocabulary.to_vec(),
        };
        zip.start_file(VOCABULARY_NAME, encrypted_options(password))?;
        zip.write_all(
            serde_json::to_string_pretty(&vocab_file)
                .context("序列化词库失败")?
                .as_bytes(),
        )?;
    }
    // PDF 本身已压缩，用 Stored 避免二次压缩浪费时间；图片同理。
    let stored_options = encrypted_stored_options(password);
    for (path, bytes) in &pdf_blobs {
        zip.start_file(path.clone(), stored_options)?;
        zip.write_all(bytes)?;
    }
    for (path, bytes) in &image_blobs {
        zip.start_file(path.clone(), stored_options)?;
        zip.write_all(bytes)?;
    }
    zip.finish()?;
    Ok(ExportSummary {
        records: manifest.records.len(),
        pdfs: total_pdfs,
    })
}

/// 把选中的稿件按选项导出为 PDF 集合，压缩进一个 zip。
///
/// - 盖章件（`stamped`）：直接取稿件附件里的 PDF；
/// - 非盖章件（`compiled`）：在临时工作目录写 TeX 并调用本机 TeX 引擎编译。
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
    let temp_path = zip_path.with_extension("tmp");
    let result = (|| -> Result<PdfExportSummary> {
        let file = File::create(&temp_path)
            .with_context(|| format!("无法创建导出文件 {}", zip_path.display()))?;
        let mut zip = ZipWriter::new(file);
        // PDF 本身已压缩，用 Stored 避免二次压缩浪费时间（与整包导出一致）。
        let stored_options = encrypted_stored_options(password);
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
            let title =
                crate::export::extract_title(&record.content_markdown, &record.snapshot.title_hint);
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
                    zip.start_file(entry, stored_options)?;
                    zip.write_all(&pdf.bytes)?;
                    wrote += 1;
                }
            }
            if options.compiled {
                match compile_record_pdf(&record, &display, &fonts, &stem) {
                    Ok(pdf_bytes) => {
                        let entry = unique_zip_name(&mut used_names, &stem, "pdf");
                        zip.start_file(entry, stored_options)?;
                        zip.write_all(&pdf_bytes)?;
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
        let file = zip.finish()?;
        drop(file);
        std::fs::rename(&temp_path, zip_path)
            .with_context(|| format!("无法写入导出文件 {}", zip_path.display()))?;
        Ok(summary)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

/// 临时工作目录计数器：与进程号组合，避免同一毫秒内并发调用碰撞。
static PDF_TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 在临时工作目录编译单篇稿件，返回生成的 PDF 字节；目录用后即删。
/// TeX 引擎未检测到时视为该篇失败（原因写入汇总，不阻断整批）。
fn compile_record_pdf(
    record: &ManuscriptRecord,
    display: &UnitDisplay,
    fonts: &FontConfig,
    stem: &str,
) -> Result<Vec<u8>> {
    let counter = PDF_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "gongwen_pdf_export_{}_{}_{}",
        std::process::id(),
        counter,
        chrono::Local::now().timestamp_millis()
    ));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("无法创建临时工作目录 {}", dir.display()))?;
    let result = (|| {
        let tex_path = dir.join(format!("{stem}.tex"));
        crate::export::write_tex(
            &tex_path,
            &record.snapshot,
            &record.content_markdown,
            display,
            fonts,
        )?;
        let pdf_path = crate::texcompile::compile_pdf_if_available(&tex_path, fonts)?
            .context("未检测到 TeX 引擎，无法编译非盖章件 PDF")?;
        std::fs::read(&pdf_path).with_context(|| format!("无法读取编译产物 {}", pdf_path.display()))
    })();
    // 清理临时目录；清理失败不掩盖编译结果。
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// 从清单记录中收集 markdown 引用的图片，返回 zip 条目（`images/<文件名>`）与字节。
/// 引用缺失或读取失败时跳过，不阻断导出。`base` 是配置目录（图片相对路径的基准）。
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
            let Ok(bytes) = std::fs::read(&source) else {
                continue;
            };
            out.push((format!("images/{}", sanitize_entry_name(file_name)), bytes));
        }
    }
    out
}

/// 只读 zip + 解析清单，不落库（导入预览用）。
pub fn read_manifest(zip_path: &Path, password: &str) -> Result<Manifest> {
    if password.is_empty() {
        bail!("请输入 ZIP 密码");
    }
    let file =
        File::open(zip_path).with_context(|| format!("无法打开导入文件 {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("文件不是有效的 ZIP")?;
    let mut reader = archive
        .by_name_decrypt(MANIFEST_NAME, password.as_bytes())
        .map_err(map_zip_password_error)
        .context("无法读取 manifests.json")?;
    let mut raw = String::new();
    reader.read_to_string(&mut raw)?;
    let manifest: Manifest = serde_json::from_str(&raw).context("manifests.json 格式无效")?;
    if manifest.schema != MANIFEST_SCHEMA {
        bail!(
            "不支持的文件格式版本：v{}（当前支持 v{}）",
            manifest.schema,
            MANIFEST_SCHEMA
        );
    }
    Ok(manifest)
}

/// 只读 zip 中的标准词库。可选条目：旧包或未附带词库的包返回 `Ok(None)`，不阻断稿件导入。
/// 只有 `vocabulary.json` 缺失时视为无词库；文件损坏或版本不支持则报错。
pub fn read_vocabulary(zip_path: &Path, password: &str) -> Result<Option<VocabularyFile>> {
    if password.is_empty() {
        bail!("请输入 ZIP 密码");
    }
    let file =
        File::open(zip_path).with_context(|| format!("无法打开导入文件 {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("文件不是有效的 ZIP")?;
    let mut reader = match archive.by_name_decrypt(VOCABULARY_NAME, password.as_bytes()) {
        Ok(reader) => reader,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(map_zip_password_error(error).into()),
    };
    if reader.size() > VOCABULARY_MAX_BYTES {
        bail!(
            "词库文件过大（{} 字节，上限 {} 字节），拒绝导入",
            reader.size(),
            VOCABULARY_MAX_BYTES
        );
    }
    // `size()` 读的是 zip 头里声明的解压后大小，是可以伪造的，所以解压时再兜一道：
    // 多读 1 字节，超了就说明声明的大小不作数，直接拒绝。
    let mut raw = String::new();
    let read = reader
        .by_ref()
        .take(VOCABULARY_MAX_BYTES + 1)
        .read_to_string(&mut raw)?;
    if read as u64 > VOCABULARY_MAX_BYTES {
        bail!(
            "词库文件解压后超过上限 {} 字节（包内声明的大小不实），拒绝导入",
            VOCABULARY_MAX_BYTES
        );
    }
    let vocab: VocabularyFile = serde_json::from_str(&raw).context("vocabulary.json 格式无效")?;
    if vocab.schema != VOCABULARY_SCHEMA {
        bail!(
            "不支持的词库格式版本：v{}（当前支持 v{}）",
            vocab.schema,
            VOCABULARY_SCHEMA
        );
    }
    Ok(Some(vocab))
}

/// 按预览勾选导入。重新读取 zip 以保证与磁盘一致；记录 id 写入 source_id 列去重。
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
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
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
            let Ok(mut entry) = archive.by_name_decrypt(&pdf.path, password.as_bytes()) else {
                summary.skipped_pdfs += 1;
                continue;
            };
            if entry.size() > MAX_PDF_BYTES {
                summary.skipped_pdfs += 1;
                continue;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
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
fn restore_images(archive: &mut ZipArchive<File>, target: &Path, password: &str) -> Result<usize> {
    std::fs::create_dir_all(target)
        .with_context(|| format!("无法创建图片目录 {}", target.display()))?;
    let names: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with("images/"))
        .map(str::to_string)
        .collect();
    let mut count = 0usize;
    for name in names {
        let file_name = name.strip_prefix("images/").unwrap_or(&name);
        let safe = sanitize_entry_name(file_name);
        let mut entry = archive
            .by_name_decrypt(&name, password.as_bytes())
            .map_err(map_zip_password_error)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        let dest = target.join(&safe);
        // 目标已存在且内容一致时跳过，避免无谓写入；内容不同则按恢复语义覆盖。
        if std::fs::read(&dest).ok().as_deref() == Some(bytes.as_slice()) {
            count += 1;
            continue;
        }
        std::fs::write(&dest, &bytes)
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
            &crate::export::extract_title(&record.content_markdown, &record.snapshot.title_hint),
        )
    }

    fn zip_names(zip_path: &Path) -> Vec<String> {
        let file = File::open(zip_path).unwrap();
        let mut names = ZipArchive::new(file)
            .unwrap()
            .file_names()
            .map(str::to_string)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn read_zip_entry(zip_path: &Path, name: &str) -> String {
        let file = File::open(zip_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut entry = archive
            .by_name_decrypt(name, TEST_PASSWORD.as_bytes())
            .unwrap();
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        text
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
            &zip_path,
            TEST_PASSWORD,
            |_| {},
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("至少选择"), "{error:#}");
    }

    #[test]
    fn export_selected_pdfs_compiled_without_engine_fails_gracefully() {
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
            &zip_path,
            TEST_PASSWORD,
            |_| {},
        )
        .unwrap();
        assert_eq!(summary.records, 1);
        // 本机有 TeX 引擎则编译成功；没有则记入失败——两种情况都不应 panic，zip 均可读。
        assert!(
            summary.pdfs == 1 || !summary.failed.is_empty(),
            "有引擎应产出 PDF，无引擎应记入失败：{summary:?}"
        );
        if let Some((_, reason)) = summary.failed.first() {
            assert!(reason.contains("TeX"), "失败原因应说明编译问题：{reason}");
        }
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
        )
        .unwrap();

        let file = File::open(&zip_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        assert!(
            (0..archive.len()).all(|index| archive.by_index_raw(index).unwrap().encrypted()),
            "导出的每一个 ZIP 条目都必须加密"
        );
        assert!(archive.by_name(MANIFEST_NAME).is_err());
        drop(archive);

        let error = read_manifest(&zip_path, "Wrong!Key8Stone").unwrap_err();
        assert!(format!("{error:#}").contains("密码错误"), "{error:#}");
        assert!(read_manifest(&zip_path, TEST_PASSWORD).is_ok());
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
        let file = File::open(&zip_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
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
        let file = File::open(&zip_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
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
            export_zip_selected(&mut store, &[second], &[], &zip_path, TEST_PASSWORD).unwrap();
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
        assert!(export_zip(&mut store, &filter, &[], &zip_path, TEST_PASSWORD).is_err());
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
