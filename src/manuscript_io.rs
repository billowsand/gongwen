//! 稿件 ZIP 导出/导入。
//!
//! ZIP 布局：`manifests.json`（带 schema 版本）+ `pdf/<id>_<序号>_<净化文件名>` 附件。
//! 导出按 `ManuscriptFilter` 过滤；导入由预览勾选 + 关键词过滤 + `skip_existing_by_id`
//! 决定写哪些记录，满足“导入也支持过滤筛选”。

use crate::manuscript::{ManuscriptFilter, ManuscriptStore, NewManuscript};
use crate::models::{DraftInput, ManuscriptStatus, TemplateKind};
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

/// 按过滤条件导出稿件（含 PDF 附件）为 ZIP。没有符合条件稿件时直接报错。
pub fn export_zip(
    store: &mut ManuscriptStore,
    filter: &ManuscriptFilter,
    zip_path: &Path,
) -> Result<ExportSummary> {
    let rows = store.list(filter)?;
    if rows.is_empty() {
        bail!("没有符合过滤条件的稿件");
    }
    let ids = rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
    export_zip_ids(store, &ids, zip_path)
}

/// 只导出稿件管理页明确勾选的记录。
pub fn export_zip_selected(
    store: &mut ManuscriptStore,
    ids: &[i64],
    zip_path: &Path,
) -> Result<ExportSummary> {
    export_zip_ids(store, ids, zip_path)
}

fn export_zip_ids(
    store: &mut ManuscriptStore,
    ids: &[i64],
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

    let file = File::create(zip_path)
        .with_context(|| format!("无法创建导出文件 {}", zip_path.display()))?;
    let mut zip = ZipWriter::new(file);
    zip.start_file(MANIFEST_NAME, SimpleFileOptions::default())?;
    zip.write_all(
        serde_json::to_string_pretty(&manifest)
            .context("序列化清单失败")?
            .as_bytes(),
    )?;
    // PDF 本身已压缩，用 Stored 避免二次压缩浪费时间。
    let pdf_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (path, bytes) in &pdf_blobs {
        zip.start_file(path.clone(), pdf_options)?;
        zip.write_all(bytes)?;
    }
    zip.finish()?;
    Ok(ExportSummary {
        records: manifest.records.len(),
        pdfs: total_pdfs,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::{ManuscriptFilter, ManuscriptStore, ManuscriptUpdate};
    use crate::models::TemplateProfile;

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
        let summary = export_zip(&mut source, &ManuscriptFilter::default(), &zip_path).unwrap();
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
    fn reimport_skips_existing_by_source_id() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("稿件.zip");
        let mut source = mem_store();
        seed(&mut source);
        export_zip(&mut source, &ManuscriptFilter::default(), &zip_path).unwrap();

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
        export_zip(&mut source, &ManuscriptFilter::default(), &zip_path).unwrap();

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

        let summary = export_zip_selected(&mut store, &[second], &zip_path).unwrap();
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
        assert!(export_zip(&mut store, &filter, &zip_path).is_err());
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
