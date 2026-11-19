//! 随文词库只取实际启用的表单字段及其上级链。
use crate::models::{DraftInput, VocabularyEntry};
use crate::units::{UnitDisplay, identity};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) const FILE_SUFFIX: &str = "-引用词库.xlsx";

pub(crate) fn referenced_entries(
    input: &DraftInput,
    display: &UnitDisplay<'_>,
    with_transmit: bool,
) -> Vec<VocabularyEntry> {
    let entries = display.entries();
    let mut selected = HashSet::new();
    for (key, category, _) in identity::active_keys(input, with_transmit) {
        if !key.starts_with("@unit:") && !key.starts_with("@person:") {
            continue;
        }
        let mut current = identity::resolve(entries, category, &key);
        while let Some(entry) = current {
            if !selected.insert(entry.key()) {
                break;
            }
            current = identity::parent(entries, entry);
        }
    }
    entries
        .iter()
        .filter(|entry| selected.contains(&entry.key()))
        .cloned()
        .collect()
}

pub(crate) fn workbook(entries: &[VocabularyEntry]) -> anyhow::Result<Option<Vec<u8>>> {
    if entries.is_empty() {
        return Ok(None);
    }
    // 归属的兼容列用本工作簿实际携带的单位编码，避免活稿补齐上级时带入旧编码。
    let mut entries = entries.to_vec();
    crate::units::identity::restore_owners(&mut entries);
    let validation = crate::vocabulary_xlsx::validate(&entries, &[]);
    anyhow::ensure!(
        validation.conflicts.is_empty(),
        "随附引用词库有 {} 项单位或人员数据冲突，无法生成可回导的 Excel",
        validation.conflicts.len()
    );
    let codes = crate::vocabulary_xlsx::unit_codes(&entries);
    crate::vocabulary_xlsx::xlsx_bytes(&entries, &codes).map(Some)
}

pub(crate) fn write_workbook(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if mdx::memory_files::output_is_captured() {
        mdx::memory_files::write_output(path, bytes)?;
    } else {
        super::protection::write_final_with(None, path, bytes)?;
    }
    Ok(())
}

/// 另存与送批材料均以用户指定的成品名称生成同目录的 Excel。
pub(crate) fn write_document_with_workbook(
    protection: Option<&super::protection::Protection>,
    document_path: &Path,
    document: Vec<u8>,
    workbook: Option<Vec<u8>>,
) -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    let Some(workbook) = workbook else {
        return Ok((
            super::protection::write_final_with(protection, document_path, &document)?,
            None,
        ));
    };
    let stem = document_path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("公文");
    let xlsx = document_path.with_file_name(format!("{stem}{FILE_SUFFIX}"));
    anyhow::ensure!(!xlsx.is_dir(), "引用词库输出位置是目录：{}", xlsx.display());
    if let Some(protection) = protection {
        let mut paths = protection.persist(
            [
                (document_path.to_owned(), document),
                (xlsx.clone(), workbook),
            ]
            .into(),
        )?;
        let document = paths
            .remove(document_path)
            .ok_or_else(|| anyhow::anyhow!("加密成品路径缺失"))?;
        let workbook = paths
            .remove(&xlsx)
            .ok_or_else(|| anyhow::anyhow!("加密引用词库路径缺失"))?;
        Ok((document, Some(workbook)))
    } else {
        let document = super::protection::write_final_with(None, document_path, &document)?;
        let workbook = super::protection::write_final_with(None, &xlsx, &workbook)?;
        Ok((document, Some(workbook)))
    }
}
