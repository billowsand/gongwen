//! 稿件 ZIP 导出/导入。
//!
//! ZIP 布局：`manifests.json`（带 schema 版本）+ `pdf/<id>_<序号>_<净化文件名>` 附件。
//! 导出按 `ManuscriptFilter` 过滤；导入由预览勾选 + 关键词过滤 + `skip_existing_by_id`
//! 决定写哪些记录，满足“导入也支持过滤筛选”。

use crate::manuscript::{ManuscriptFilter, ManuscriptStore, NewManuscript};
use crate::models::{DraftInput, ManuscriptStatus, TemplateKind, VocabularyEntry};
use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use zip::read::ZipArchive;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const MANIFEST_SCHEMA: u32 = 1;
const MANIFEST_NAME: &str = "manifests.json";
/// 随稿件包导出的标准词库（全局一份，随包带走，导入时增量合并）。
pub const VOCABULARY_SCHEMA: u32 = 1;
const VOCABULARY_NAME: &str = "vocabulary.json";
/// 词库 JSON 体积上限（约 10 MB），远超正常词库规模，防恶意包撑爆内存。
const VOCABULARY_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 单个 PDF 附件上限（约 100 MB），超限跳过，避免导入超大文件撑爆库。
const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;

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
) -> Result<ExportSummary> {
    let rows = store.list(filter)?;
    if rows.is_empty() {
        bail!("没有符合过滤条件的稿件");
    }
    let ids = rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
    export_zip_ids(store, &ids, vocabulary, zip_path)
}

/// 只导出稿件管理页明确勾选的记录。
pub fn export_zip_selected(
    store: &mut ManuscriptStore,
    ids: &[i64],
    vocabulary: &[VocabularyEntry],
    zip_path: &Path,
) -> Result<ExportSummary> {
    export_zip_ids(store, ids, vocabulary, zip_path)
}

fn export_zip_ids(
    store: &mut ManuscriptStore,
    ids: &[i64],
    vocabulary: &[VocabularyEntry],
    zip_path: &Path,
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
    zip.start_file(MANIFEST_NAME, SimpleFileOptions::default())?;
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
        zip.start_file(VOCABULARY_NAME, SimpleFileOptions::default())?;
        zip.write_all(
            serde_json::to_string_pretty(&vocab_file)
                .context("序列化词库失败")?
                .as_bytes(),
        )?;
    }
    // PDF 本身已压缩，用 Stored 避免二次压缩浪费时间；图片同理。
    let stored_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
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
pub fn read_manifest(zip_path: &Path) -> Result<Manifest> {
    let file =
        File::open(zip_path).with_context(|| format!("无法打开导入文件 {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("文件不是有效的 ZIP")?;
    let mut reader = archive
        .by_name(MANIFEST_NAME)
        .context("ZIP 中缺少 manifests.json")?;
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
pub fn read_vocabulary(zip_path: &Path) -> Result<Option<VocabularyFile>> {
    let file =
        File::open(zip_path).with_context(|| format!("无法打开导入文件 {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("文件不是有效的 ZIP")?;
    let mut reader = match archive.by_name(VOCABULARY_NAME) {
        Ok(reader) => reader,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
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
) -> Result<ImportSummary> {
    let manifest = read_manifest(zip_path)?;
    if opts.selected.len() != manifest.records.len() {
        bail!("勾选状态与清单记录数不一致，请重新预览");
    }
    let mut summary = ImportSummary::default();
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    // 恢复图片资源：图片是跨稿件共享目录，全量解压；旧包无 images/ 条目时无操作。
    if let Ok(image_dir) = crate::images::image_dir() {
        restore_images(&mut archive, &image_dir)?;
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
            let Ok(mut entry) = archive.by_name(&pdf.path) else {
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

/// 从 zip 恢复 `images/` 条目到目标目录。条目名经过净化，防止篡改的 zip
/// 用路径穿越覆盖任意文件；返回恢复的文件数。
fn restore_images(archive: &mut ZipArchive<File>, target: &Path) -> Result<usize> {
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
        let mut entry = archive.by_name(&name)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::{ManuscriptFilter, ManuscriptStore, ManuscriptUpdate};
    use crate::models::{TemplateProfile, VocabularyCategory};

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

    #[test]
    fn export_import_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("稿件.zip");

        let mut source = mem_store();
        let id = seed(&mut source);
        let summary =
            export_zip(&mut source, &ManuscriptFilter::default(), &[], &zip_path).unwrap();
        assert_eq!(summary.records, 1);
        assert_eq!(summary.pdfs, 2);

        // 导出后源库记录与清单记录 id 一致。
        let manifest = read_manifest(&zip_path).unwrap();
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
        let result = import_zip(&mut dest, &zip_path, &opts).unwrap();
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
        )
        .unwrap();

        let read = read_vocabulary(&zip_path).unwrap().expect("包内应带词库");
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
        export_zip(&mut store, &ManuscriptFilter::default(), &[], &zip_path).unwrap();
        assert!(read_vocabulary(&zip_path).unwrap().is_none());
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
        assert!(read_vocabulary(&zip_path).is_err());
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
        let error = read_vocabulary(&zip_path).unwrap_err();
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
        let count = restore_images(&mut archive, target.path()).unwrap();
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
        assert_eq!(restore_images(&mut archive, target.path()).unwrap(), 2);
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
        export_zip(&mut source, &ManuscriptFilter::default(), &[], &zip_path).unwrap();

        let mut dest = mem_store();
        let opts = ImportOptions {
            skip_existing_by_id: true,
            selected: vec![true],
        };
        import_zip(&mut dest, &zip_path, &opts).unwrap();
        let second = import_zip(&mut dest, &zip_path, &opts).unwrap();
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
        export_zip(&mut source, &ManuscriptFilter::default(), &[], &zip_path).unwrap();

        let mut dest = mem_store();
        let opts = ImportOptions {
            skip_existing_by_id: true,
            selected: vec![false],
        };
        let result = import_zip(&mut dest, &zip_path, &opts).unwrap();
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

        let summary = export_zip_selected(&mut store, &[second], &[], &zip_path).unwrap();
        assert_eq!(summary.records, 1);
        let manifest = read_manifest(&zip_path).unwrap();
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
        assert!(export_zip(&mut store, &filter, &[], &zip_path).is_err());
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
        assert!(read_manifest(&zip_path).is_err());
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize_entry_name("扫描件.pdf"), "扫描件.pdf");
        assert_eq!(sanitize_entry_name("a/b\\c:d*e?.pdf"), "a_b_c_d_e_.pdf");
        assert_eq!(sanitize_entry_name(""), "pdf.pdf");
    }
}
