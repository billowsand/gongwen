//! 导入包的逐篇身份判定与共同祖先查找。
use super::{Manifest, ManifestRecord};
use crate::manuscript::{
    ManuscriptStore,
    merge::MergeProposal,
    sync::{SyncRevision, payload_hash},
};
use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relationship {
    New,
    Same,
    Equivalent,
    LocalAhead,
    IncomingAhead,
    Diverged,
    NoBase,
    Legacy,
    Archived,
}

impl Relationship {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "新稿件",
            Self::Same => "同一版本",
            Self::Equivalent => "内容相同、历史不同",
            Self::LocalAhead => "本机较新",
            Self::IncomingAhead => "导入版较新",
            Self::Diverged => "双方已分叉",
            Self::NoBase => "无共同基线",
            Self::Legacy => "旧版包，身份待确认",
            Self::Archived => "本机已归档",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordPreview {
    pub relationship: Relationship,
    pub local_id: Option<i64>,
    pub local_status: Option<crate::models::ManuscriptStatus>,
    pub attachments_changed: bool,
    pub candidates: Vec<i64>,
    pub base: Option<SyncRevision>,
    pub proposal: Option<MergeProposal>,
    pub local_hash: Option<String>,
    pub incoming_hash: String,
}

pub fn inspect(store: &mut ManuscriptStore, manifest: &Manifest) -> Result<Vec<RecordPreview>> {
    manifest
        .records
        .iter()
        .map(|record| inspect_one(store, manifest.schema, record))
        .collect()
}

pub fn inspect_one(
    store: &mut ManuscriptStore,
    schema: u32,
    record: &ManifestRecord,
) -> Result<RecordPreview> {
    let incoming_hash = payload_hash(&record.snapshot, &record.content_markdown, &record.notes)?;
    let candidates = store
        .list(&Default::default())?
        .into_iter()
        .filter(|row| {
            row.title == record.title
                || !record.doc_number.is_empty() && row.doc_number == record.doc_number
        })
        .map(|row| row.id)
        .collect::<Vec<_>>();
    let Some(document_uuid) = record.document_uuid.as_deref() else {
        let fingerprint = legacy_fingerprint(record)?;
        let existing = store.find_legacy_import(&fingerprint)?;
        return Ok(RecordPreview {
            relationship: if existing.is_some() {
                Relationship::Same
            } else {
                Relationship::Legacy
            },
            local_id: existing,
            local_status: None,
            attachments_changed: false,
            candidates,
            base: None,
            proposal: None,
            local_hash: None,
            incoming_hash,
        });
    };
    if schema == 1 {
        bail!("旧版包不得声称带有新版稿件身份");
    }
    let mut matches = HashSet::new();
    for identity in std::iter::once(document_uuid).chain(record.aliases.iter().map(String::as_str))
    {
        if let Some(id) = store.find_document_uuid(identity)? {
            matches.insert(id);
        }
    }
    if matches.len() > 1 {
        bail!("包内稿件身份分别指向本机多篇稿件，不能自动关联");
    }
    let Some(local_id) = matches.into_iter().next() else {
        return Ok(RecordPreview {
            relationship: Relationship::New,
            local_id: None,
            local_status: None,
            attachments_changed: false,
            candidates,
            base: None,
            proposal: None,
            local_hash: None,
            incoming_hash,
        });
    };
    let local = store.get(local_id)?.expect("稿件身份应对应记录");
    let local_hash = payload_hash(&local.snapshot, &local.content_markdown, &local.notes)?;
    let local_pdf_hashes = local
        .pdfs
        .iter()
        .map(|pdf| super::bytes_hash(&pdf.bytes))
        .collect::<HashSet<_>>();
    let attachments_changed = record.pdfs.iter().any(|pdf| {
        pdf.sha256
            .as_ref()
            .is_some_and(|hash| !local_pdf_hashes.contains(hash))
    });
    let mut graph = HashMap::<String, SyncRevision>::new();
    for revision in store
        .sync_revisions(local_id)?
        .into_iter()
        .chain(record.revisions.iter().cloned())
    {
        if let Some(old) = graph.insert(revision.revision_uuid.clone(), revision.clone())
            && old.payload_hash != revision.payload_hash
        {
            bail!("相同版本身份对应不同内容：{}", revision.revision_uuid);
        }
    }
    let (_, local_head) = store.document_identity(local_id)?;
    let remote_head = record.head_revision_uuid.as_deref().expect("v2 已校验");
    let clean = local_head
        .as_ref()
        .and_then(|head| graph.get(head))
        .is_some_and(|revision| revision.payload_hash == local_hash);
    let relationship = if local.status == crate::models::ManuscriptStatus::Archived
        && (local_hash != incoming_hash || local.status != record.status)
    {
        Relationship::Archived
    } else if local_hash == incoming_hash {
        if clean && local_head.as_deref() == Some(remote_head) {
            Relationship::Same
        } else {
            Relationship::Equivalent
        }
    } else if let Some(ref head) = local_head {
        if clean && is_ancestor(head, remote_head, &graph) {
            Relationship::IncomingAhead
        } else if is_ancestor(remote_head, head, &graph) {
            Relationship::LocalAhead
        } else {
            Relationship::Diverged
        }
    } else {
        Relationship::NoBase
    };
    let mut base = None;
    let mut proposal = None;
    let relationship = if relationship == Relationship::Diverged {
        if let Some(head) = local_head.as_deref() {
            let lcas = common_bases(head, remote_head, &graph);
            if lcas.len() == 1 {
                base = graph.get(&lcas[0]).cloned();
                if let Some(ref ancestor) = base {
                    proposal = Some(MergeProposal::build(
                        (
                            &ancestor.snapshot,
                            &ancestor.content_markdown,
                            &ancestor.notes,
                        ),
                        (&local.snapshot, &local.content_markdown, &local.notes),
                        (&record.snapshot, &record.content_markdown, &record.notes),
                    )?);
                }
                Relationship::Diverged
            } else {
                Relationship::NoBase
            }
        } else {
            Relationship::NoBase
        }
    } else {
        relationship
    };
    Ok(RecordPreview {
        relationship,
        local_id: Some(local_id),
        local_status: Some(local.status),
        attachments_changed,
        candidates,
        base,
        proposal,
        local_hash: Some(local_hash),
        incoming_hash,
    })
}

fn ancestors(head: &str, graph: &HashMap<String, SyncRevision>) -> HashSet<String> {
    let mut found = HashSet::new();
    let mut stack = vec![head.to_string()];
    while let Some(id) = stack.pop() {
        if found.insert(id.clone())
            && let Some(node) = graph.get(&id)
        {
            stack.extend(node.parents.iter().cloned());
        }
    }
    found
}

fn is_ancestor(older: &str, newer: &str, graph: &HashMap<String, SyncRevision>) -> bool {
    ancestors(newer, graph).contains(older)
}

fn common_bases(local: &str, incoming: &str, graph: &HashMap<String, SyncRevision>) -> Vec<String> {
    let shared = ancestors(local, graph)
        .intersection(&ancestors(incoming, graph))
        .cloned()
        .collect::<Vec<_>>();
    shared
        .iter()
        .filter(|candidate| {
            !shared
                .iter()
                .any(|other| *other != **candidate && is_ancestor(candidate, other, graph))
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone)]
pub struct PendingPreview {
    pub head: String,
    pub base: Option<SyncRevision>,
    pub incoming: SyncRevision,
    pub imported_status: Option<crate::models::ManuscriptStatus>,
    pub proposal: MergeProposal,
}

pub fn inspect_pending(store: &mut ManuscriptStore, id: i64, head: &str) -> Result<PendingPreview> {
    let current = store.get(id)?.context("稿件不存在")?;
    let graph = store
        .sync_revisions(id)?
        .into_iter()
        .map(|revision| (revision.revision_uuid.clone(), revision))
        .collect::<HashMap<_, _>>();
    let incoming = graph.get(head).cloned().context("待处理分支不存在")?;
    let (_, local_head) = store.document_identity(id)?;
    let base = local_head.as_deref().and_then(|local_head| {
        let bases = common_bases(local_head, head, &graph);
        if bases.len() == 1 {
            graph.get(&bases[0]).cloned()
        } else {
            None
        }
    });
    let proposal = if let Some(ancestor) = &base {
        MergeProposal::build(
            (
                &ancestor.snapshot,
                &ancestor.content_markdown,
                &ancestor.notes,
            ),
            (&current.snapshot, &current.content_markdown, &current.notes),
            (
                &incoming.snapshot,
                &incoming.content_markdown,
                &incoming.notes,
            ),
        )?
    } else {
        MergeProposal::manual(
            (&current.snapshot, &current.content_markdown, &current.notes),
            (
                &incoming.snapshot,
                &incoming.content_markdown,
                &incoming.notes,
            ),
        )?
    };
    let imported_status = store.pending_sync_status(id, head)?.and_then(|label| {
        crate::models::ManuscriptStatus::ALL
            .into_iter()
            .find(|status| status.label() == label)
    });
    Ok(PendingPreview {
        head: head.to_string(),
        base,
        incoming,
        imported_status,
        proposal,
    })
}

pub fn merge_pending(
    store: &mut ManuscriptStore,
    id: i64,
    head: &str,
    choices: &[bool],
    markdown_override: Option<&str>,
    take_status: bool,
) -> Result<()> {
    use crate::manuscript::ManuscriptUpdate;
    let preview = inspect_pending(store, id, head)?;
    let current = store.get(id)?.context("稿件不存在")?;
    if current.status == crate::models::ManuscriptStatus::Archived {
        bail!("归档稿件不可合并");
    }
    let (snapshot, mut markdown, notes) = preview.proposal.resolve(choices)?;
    if let Some(edited) = markdown_override {
        markdown = edited.to_string();
    }
    store.begin_sync_import()?;
    let result = (|| -> Result<()> {
        let local_head = store.sync_checkpoint(id)?.revision_uuid;
        store.update(
            id,
            &ManuscriptUpdate {
                snapshot: snapshot.clone(),
                content_markdown: markdown.clone(),
                notes: notes.clone(),
            },
        )?;
        store.commit_merged_sync_revision(
            id,
            [local_head, head.to_string()],
            &snapshot,
            &markdown,
            &notes,
        )?;
        if take_status && let Some(status) = preview.imported_status {
            apply_status(store, id, status)?;
        }
        store.remove_pending_sync_head(id, head)?;
        Ok(())
    })();
    match result {
        Ok(()) => store.commit_sync_import(),
        Err(error) => {
            let _ = store.rollback_sync_import();
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DraftInput;

    #[test]
    fn merge_base_is_shared_ancestor_not_latest_number() {
        let draft = DraftInput::default();
        let root = SyncRevision::new(
            vec![],
            draft.clone(),
            "甲".into(),
            "".into(),
            "".into(),
            "".into(),
            None,
        )
        .unwrap();
        let left = SyncRevision::new(
            vec![root.revision_uuid.clone()],
            draft.clone(),
            "甲本".into(),
            "".into(),
            "".into(),
            "".into(),
            None,
        )
        .unwrap();
        let right = SyncRevision::new(
            vec![root.revision_uuid.clone()],
            draft,
            "甲外".into(),
            "".into(),
            "".into(),
            "".into(),
            None,
        )
        .unwrap();
        let graph = [root.clone(), left.clone(), right.clone()]
            .into_iter()
            .map(|r| (r.revision_uuid.clone(), r))
            .collect();
        assert_eq!(
            common_bases(&left.revision_uuid, &right.revision_uuid, &graph),
            vec![root.revision_uuid]
        );
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportAction {
    Skip,
    New,
    UseIncoming {
        take_status: bool,
    },
    Merge {
        choices: Vec<bool>,
        markdown_override: Option<String>,
        take_status: bool,
    },
    Pending,
    LinkPending {
        local_id: i64,
    },
    Copy,
}

#[derive(Debug, Default)]
pub struct SyncImportSummary {
    pub created: usize,
    pub updated: usize,
    pub merged: usize,
    pub pending: usize,
    pub skipped: usize,
    pub pdfs_added: usize,
    pub changed_ids: Vec<i64>,
}

pub fn legacy_fingerprint(record: &ManifestRecord) -> Result<String> {
    Ok(super::bytes_hash(&serde_json::to_vec(record)?))
}

fn link_aliases(store: &ManuscriptStore, id: i64, record: &ManifestRecord) -> Result<()> {
    if let Some(uuid) = &record.document_uuid {
        store.link_document_alias(id, uuid)?;
    }
    for alias in &record.aliases {
        store.link_document_alias(id, alias)?;
    }
    Ok(())
}

type Blob = (String, Vec<u8>);

/// 校验预览后重新读取的同一份清单和所有资源，再整体写入。
pub fn import_with_actions(
    store: &mut ManuscriptStore,
    zip_path: &std::path::Path,
    password: &str,
    expected_manifest_hash: &str,
    actions: &[ImportAction],
) -> Result<SyncImportSummary> {
    use super::{MAX_PDF_BYTES, read_manifest};
    use crate::manuscript::ManuscriptUpdate;
    let manifest = read_manifest(zip_path, password)?;
    let actual_hash = super::bytes_hash(&serde_json::to_vec(&manifest)?);
    if actual_hash != expected_manifest_hash || actions.len() != manifest.records.len() {
        bail!("稿件包自预览后发生变化，请重新预览");
    }
    let before = inspect(store, &manifest)?;
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let images = read_images(&mut archive, &manifest, password, actions)?;
    let mut pdfs = Vec::<Vec<(String, Vec<u8>)>>::new();
    for (record, action) in manifest.records.iter().zip(actions) {
        let mut record_pdfs = Vec::new();
        if !matches!(action, ImportAction::Skip) {
            for pdf in &record.pdfs {
                match read_blob(&mut archive, &pdf.path, password, MAX_PDF_BYTES) {
                    Ok(bytes) => {
                        if let Some(expected) = &pdf.sha256
                            && super::bytes_hash(&bytes) != *expected
                        {
                            bail!("PDF 附件校验失败：{}", pdf.file_name);
                        }
                        record_pdfs.push((pdf.file_name.clone(), bytes));
                    }
                    Err(error) if manifest.schema == 1 => {
                        let _ = error;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        pdfs.push(record_pdfs);
    }
    // 资源名冲突必须在写入前发现，绝不覆盖本机可能被其他稿件引用的文件。
    let image_dir = crate::images::image_dir()?;
    for (path, bytes) in &images {
        let dest = crate::images::resolve_from_base(&crate::storage::config_dir()?, path)?;
        if dest.exists() && std::fs::read(&dest)? != *bytes {
            bail!("图片同名但内容不同，请先处理资源冲突：{path}");
        }
    }
    store.begin_sync_import()?;
    let mut created_files = Vec::new();
    let result = (|| -> Result<SyncImportSummary> {
        let mut summary = SyncImportSummary::default();
        for (index, ((record, action), preview)) in manifest
            .records
            .iter()
            .zip(actions)
            .zip(&before)
            .enumerate()
        {
            let id = match action {
                ImportAction::Skip => {
                    summary.skipped += 1;
                    continue;
                }
                ImportAction::New => {
                    if manifest.schema == 2 && preview.relationship != Relationship::New
                        || manifest.schema == 1 && preview.relationship != Relationship::Legacy
                    {
                        bail!("稿件关系已变化，请重新预览");
                    }
                    let id = create_record(store, record, None)?;
                    if let Some(uuid) = &record.document_uuid {
                        store.assign_document_uuid(id, uuid)?;
                        link_aliases(store, id, record)?;
                        store.insert_sync_revisions(id, &record.revisions)?;
                        let head = record.head_revision_uuid.as_deref().expect("v2 已校验");
                        store.set_sync_head(id, head)?;
                        store.materialize_imported_versions(id, &record.revisions)?;
                    } else {
                        store.sync_checkpoint(id)?;
                        store.record_legacy_import(&legacy_fingerprint(record)?, id)?;
                    }
                    summary.created += 1;
                    id
                }
                ImportAction::Copy => {
                    let id =
                        create_record(store, record, Some(crate::models::ManuscriptStatus::Draft))?;
                    store.sync_checkpoint(id)?;
                    summary.created += 1;
                    id
                }
                ImportAction::UseIncoming { take_status } => {
                    if !matches!(
                        preview.relationship,
                        Relationship::IncomingAhead | Relationship::Equivalent | Relationship::Same
                    ) {
                        bail!("导入版已不是可直接更新的版本，请重新预览");
                    }
                    let id = preview.local_id.expect("已有稿件");
                    let local_head = store.sync_checkpoint(id)?.revision_uuid;
                    link_aliases(store, id, record)?;
                    store.insert_sync_revisions(id, &record.revisions)?;
                    let incoming_head = record.head_revision_uuid.as_deref().expect("v2 已校验");
                    if preview.relationship == Relationship::IncomingAhead {
                        store.update(
                            id,
                            &ManuscriptUpdate {
                                snapshot: record.snapshot.clone(),
                                content_markdown: record.content_markdown.clone(),
                                notes: record.notes.clone(),
                            },
                        )?;
                        store.set_sync_head(id, incoming_head)?;
                    } else {
                        store.join_equivalent_sync_heads(
                            id,
                            local_head,
                            incoming_head.to_string(),
                            &record.snapshot,
                            &record.content_markdown,
                            &record.notes,
                        )?;
                    }
                    if *take_status {
                        apply_status(store, id, record.status)?;
                    }
                    store.materialize_imported_versions(id, &record.revisions)?;
                    summary.updated += 1;
                    summary.changed_ids.push(id);
                    id
                }
                ImportAction::Merge {
                    choices,
                    markdown_override,
                    take_status,
                } => {
                    if preview.relationship != Relationship::Diverged {
                        bail!("共同基线已变化，请重新预览");
                    }
                    let id = preview.local_id.expect("已有稿件");
                    let proposal = preview.proposal.as_ref().expect("唯一共同基线");
                    let (snapshot, mut markdown, notes) = proposal.resolve(choices)?;
                    if let Some(edited) = markdown_override {
                        markdown.clone_from(edited);
                    }
                    let local_head = store.sync_checkpoint(id)?.revision_uuid;
                    link_aliases(store, id, record)?;
                    store.insert_sync_revisions(id, &record.revisions)?;
                    let incoming_head = record.head_revision_uuid.clone().expect("v2 已校验");
                    store.update(
                        id,
                        &ManuscriptUpdate {
                            snapshot: snapshot.clone(),
                            content_markdown: markdown.clone(),
                            notes: notes.clone(),
                        },
                    )?;
                    store.commit_merged_sync_revision(
                        id,
                        [local_head, incoming_head],
                        &snapshot,
                        &markdown,
                        &notes,
                    )?;
                    if *take_status {
                        apply_status(store, id, record.status)?;
                    }
                    store.materialize_imported_versions(id, &record.revisions)?;
                    summary.merged += 1;
                    summary.changed_ids.push(id);
                    id
                }
                ImportAction::Pending => {
                    let id = preview.local_id.context("没有可暂存分支的本机稿件")?;
                    link_aliases(store, id, record)?;
                    store.insert_sync_revisions(id, &record.revisions)?;
                    store.save_pending_sync_head(
                        id,
                        record.head_revision_uuid.as_deref().expect("v2 已校验"),
                        record.status.label(),
                    )?;
                    summary.pending += 1;
                    id
                }
                ImportAction::LinkPending { local_id } => {
                    let local = store.get(*local_id)?.context("关联目标稿件不存在")?;
                    if local.status == crate::models::ManuscriptStatus::Archived {
                        bail!("已归档稿件不能关联待合并内容，请另存副本");
                    }
                    let id = *local_id;
                    if let Some(uuid) = &record.document_uuid {
                        let _ = uuid;
                        link_aliases(store, id, record)?;
                        store.insert_sync_revisions(id, &record.revisions)?;
                        store.save_pending_sync_head(
                            id,
                            record.head_revision_uuid.as_deref().expect("v2 已校验"),
                            record.status.label(),
                        )?;
                    } else {
                        let revision = SyncRevision::new(
                            vec![],
                            record.snapshot.clone(),
                            record.content_markdown.clone(),
                            record.notes.clone(),
                            "旧版包导入".into(),
                            String::new(),
                            None,
                        )?;
                        store.insert_sync_revisions(id, std::slice::from_ref(&revision))?;
                        store.save_pending_sync_head(
                            id,
                            &revision.revision_uuid,
                            record.status.label(),
                        )?;
                        store.record_legacy_import(&legacy_fingerprint(record)?, id)?;
                    }
                    summary.pending += 1;
                    id
                }
            };
            for (file_name, bytes) in &pdfs[index] {
                let present = store
                    .get(id)?
                    .expect("刚导入的稿件")
                    .pdfs
                    .iter()
                    .any(|pdf| super::bytes_hash(&pdf.bytes) == super::bytes_hash(bytes));
                if !present {
                    let same_name = store
                        .get(id)?
                        .expect("刚导入的稿件")
                        .pdfs
                        .iter()
                        .any(|pdf| pdf.file_name == *file_name);
                    let name = if same_name {
                        format!("{}（导入）", file_name)
                    } else {
                        file_name.clone()
                    };
                    store.add_pdf(id, &name, bytes)?;
                    summary.pdfs_added += 1;
                }
            }
        }
        std::fs::create_dir_all(&image_dir)?;
        for (path, bytes) in &images {
            let dest = crate::images::resolve_from_base(&crate::storage::config_dir()?, path)?;
            if !dest.exists() {
                std::fs::write(&dest, bytes)?;
                created_files.push(dest);
            }
        }
        Ok(summary)
    })();
    match result {
        Ok(summary) => {
            if let Err(error) = store.commit_sync_import() {
                for path in created_files {
                    let _ = std::fs::remove_file(path);
                }
                return Err(error);
            }
            Ok(summary)
        }
        Err(error) => {
            let _ = store.rollback_sync_import();
            for path in created_files {
                let _ = std::fs::remove_file(path);
            }
            Err(error)
        }
    }
}

fn create_record(
    store: &mut ManuscriptStore,
    record: &ManifestRecord,
    status_override: Option<crate::models::ManuscriptStatus>,
) -> Result<i64> {
    use crate::manuscript::NewManuscript;
    store.create(
        &NewManuscript {
            snapshot: record.snapshot.clone(),
            content_markdown: record.content_markdown.clone(),
            notes: record.notes.clone(),
            status: status_override.unwrap_or(record.status),
            created_at: Some(record.created_at.clone()),
            updated_at: Some(record.updated_at.clone()),
            published_at: if status_override.is_none() {
                record.published_at.clone()
            } else {
                None
            },
            archived_at: if status_override.is_none() {
                record.archived_at.clone()
            } else {
                None
            },
        },
        None,
    )
}

fn apply_status(
    store: &mut ManuscriptStore,
    id: i64,
    incoming: crate::models::ManuscriptStatus,
) -> Result<()> {
    let current = store.get(id)?.context("稿件不存在")?.status;
    if current != incoming {
        store.set_status(id, incoming)?;
    }
    Ok(())
}

fn read_blob(
    archive: &mut zip::ZipArchive<std::fs::File>,
    path: &str,
    password: &str,
    max: u64,
) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut entry = archive
        .by_name_decrypt(path, password.as_bytes())
        .map_err(super::map_zip_password_error)?;
    if entry.size() > max {
        bail!("资源文件过大：{path}");
    }
    let mut bytes = Vec::new();
    entry.by_ref().take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        bail!("资源解压后超过大小上限：{path}");
    }
    Ok(bytes)
}

fn read_images(
    archive: &mut zip::ZipArchive<std::fs::File>,
    manifest: &Manifest,
    password: &str,
    actions: &[ImportAction],
) -> Result<Vec<Blob>> {
    let mut out = Vec::new();
    if manifest.schema == 2 {
        let mut needed = HashSet::new();
        for (record, action) in manifest.records.iter().zip(actions) {
            if matches!(action, ImportAction::Skip) {
                continue;
            }
            for markdown in std::iter::once(&record.content_markdown).chain(
                record
                    .revisions
                    .iter()
                    .map(|revision| &revision.content_markdown),
            ) {
                for src in crate::images::image_refs(markdown) {
                    if src.starts_with("images/") {
                        needed.insert(src);
                    }
                }
            }
        }
        for asset in &manifest.assets {
            if !needed.contains(&asset.path) {
                continue;
            }
            let bytes = read_blob(archive, &asset.path, password, super::MAX_PDF_BYTES)?;
            if super::bytes_hash(&bytes) != asset.sha256 {
                bail!("图片资源校验失败：{}", asset.path);
            }
            out.push((asset.path.clone(), bytes));
        }
    } else {
        let names = archive
            .file_names()
            .filter(|name| name.starts_with("images/"))
            .map(str::to_string)
            .collect::<Vec<_>>();
        for path in names {
            if !safe_image_path(&path) {
                bail!("旧版包图片路径不安全：{path}");
            }
            out.push((
                path.clone(),
                read_blob(archive, &path, password, super::MAX_PDF_BYTES)?,
            ));
        }
    }
    Ok(out)
}

fn safe_image_path(path: &str) -> bool {
    path.starts_with("images/")
        && !path.contains('\\')
        && path.split('/').count() == 2
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

#[cfg(test)]
mod round_trip_tests {
    use super::*;
    use crate::manuscript::{ManuscriptUpdate, NewManuscript};
    use crate::models::{DraftInput, ManuscriptStatus};
    use std::path::Path;

    const PASSWORD: &str = "Jade!River7Cloud";

    fn create(store: &mut ManuscriptStore, body: &str, title: &str) -> i64 {
        let snapshot = DraftInput {
            title_hint: title.to_string(),
            ..Default::default()
        };
        store
            .create(
                &NewManuscript {
                    snapshot,
                    content_markdown: body.to_string(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap()
    }

    fn export(store: &mut ManuscriptStore, id: i64, path: &Path) {
        super::super::export_zip_selected(store, &[id], &[], path, PASSWORD).unwrap();
    }

    fn import(
        store: &mut ManuscriptStore,
        path: &Path,
        actions: Vec<ImportAction>,
    ) -> Result<SyncImportSummary> {
        let manifest = super::super::read_manifest(path, PASSWORD)?;
        let hash = super::super::bytes_hash(&serde_json::to_vec(&manifest)?);
        import_with_actions(store, path, PASSWORD, &hash, &actions)
    }

    #[test]
    fn two_databases_round_trip_and_merge_disjoint_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲。\n乙。\n", "往返稿");
        let first = dir.path().join("first.zip");
        export(&mut a, id_a, &first);
        let manifest = super::super::read_manifest(&first, PASSWORD).unwrap();
        assert_eq!(
            inspect(&mut b, &manifest).unwrap()[0].relationship,
            Relationship::New
        );
        let imported = import(&mut b, &first, vec![ImportAction::New]).unwrap();
        assert_eq!(imported.created, 1);
        let id_b = b
            .find_document_uuid(manifest.records[0].document_uuid.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(id_a, id_b);
        assert_eq!(
            inspect(&mut b, &manifest).unwrap()[0].relationship,
            Relationship::Same
        );
        let repeated = import(&mut b, &first, vec![ImportAction::Skip]).unwrap();
        assert_eq!(repeated.skipped, 1);
        assert_eq!(b.list(&Default::default()).unwrap().len(), 1);

        let original = b.get(id_b).unwrap().unwrap();
        b.update(
            id_b,
            &ManuscriptUpdate {
                snapshot: original.snapshot,
                content_markdown: "甲。\n乙改。\n".into(),
                notes: original.notes,
            },
        )
        .unwrap();
        let second = dir.path().join("second.zip");
        export(&mut b, id_b, &second);
        let second_manifest = super::super::read_manifest(&second, PASSWORD).unwrap();
        assert_eq!(
            inspect(&mut a, &second_manifest).unwrap()[0].relationship,
            Relationship::IncomingAhead
        );
        import(
            &mut a,
            &second,
            vec![ImportAction::UseIncoming { take_status: false }],
        )
        .unwrap();
        assert_eq!(
            a.get(id_a).unwrap().unwrap().content_markdown,
            "甲。\n乙改。\n"
        );

        let old_a = a.get(id_a).unwrap().unwrap();
        a.update(
            id_a,
            &ManuscriptUpdate {
                snapshot: old_a.snapshot,
                content_markdown: "甲本。\n乙改。\n".into(),
                notes: old_a.notes,
            },
        )
        .unwrap();
        let old_b = b.get(id_b).unwrap().unwrap();
        b.update(
            id_b,
            &ManuscriptUpdate {
                snapshot: old_b.snapshot,
                content_markdown: "甲。\n乙外。\n".into(),
                notes: old_b.notes,
            },
        )
        .unwrap();
        let third = dir.path().join("third.zip");
        export(&mut b, id_b, &third);
        let third_manifest = super::super::read_manifest(&third, PASSWORD).unwrap();
        let preview = inspect(&mut a, &third_manifest).unwrap();
        assert_eq!(preview[0].relationship, Relationship::Diverged);
        assert_eq!(preview[0].proposal.as_ref().unwrap().conflict_count(), 0);
        import(
            &mut a,
            &third,
            vec![ImportAction::Merge {
                choices: vec![],
                markdown_override: None,
                take_status: false,
            }],
        )
        .unwrap();
        assert_eq!(
            a.get(id_a).unwrap().unwrap().content_markdown,
            "甲本。\n乙外。\n"
        );
        let (_, head) = a.document_identity(id_a).unwrap();
        let merge = a
            .sync_revisions(id_a)
            .unwrap()
            .into_iter()
            .find(|revision| Some(&revision.revision_uuid) == head.as_ref())
            .unwrap();
        assert_eq!(merge.parents.len(), 2);
    }

    #[test]
    fn equal_integer_ids_do_not_claim_same_manuscript() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲。", "甲稿");
        let id_b = create(&mut b, "乙。", "乙稿");
        assert_eq!(id_a, id_b);
        let zip = dir.path().join("a.zip");
        export(&mut a, id_a, &zip);
        let manifest = super::super::read_manifest(&zip, PASSWORD).unwrap();
        assert_eq!(
            inspect(&mut b, &manifest).unwrap()[0].relationship,
            Relationship::New
        );
        import(&mut b, &zip, vec![ImportAction::New]).unwrap();
        assert_eq!(b.list(&Default::default()).unwrap().len(), 2);
    }

    #[test]
    fn same_revision_uuid_with_different_payload_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ManuscriptStore::open(&dir.path().join("db.sqlite")).unwrap();
        let id = create(&mut store, "甲", "甲稿");
        let first = store.sync_checkpoint(id).unwrap();
        let mut forged = SyncRevision::new(
            vec![],
            DraftInput::default(),
            "乙".into(),
            "".into(),
            "".into(),
            "".into(),
            None,
        )
        .unwrap();
        forged.revision_uuid = first.revision_uuid;
        assert!(store.insert_sync_revisions(id, &[forged]).is_err());
    }

    fn update_body(store: &mut ManuscriptStore, id: i64, body: &str) {
        let record = store.get(id).unwrap().unwrap();
        store
            .update(
                id,
                &ManuscriptUpdate {
                    snapshot: record.snapshot,
                    content_markdown: body.to_string(),
                    notes: record.notes,
                },
            )
            .unwrap();
    }

    fn update_snapshot(store: &mut ManuscriptStore, id: i64, edit: impl FnOnce(&mut DraftInput)) {
        let record = store.get(id).unwrap().unwrap();
        let mut snapshot = record.snapshot;
        edit(&mut snapshot);
        store
            .update(
                id,
                &ManuscriptUpdate {
                    snapshot,
                    content_markdown: record.content_markdown,
                    notes: record.notes,
                },
            )
            .unwrap();
    }

    #[test]
    fn local_ahead_is_reported_and_import_keeps_local_content() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲。\n", "往返稿");
        let first = dir.path().join("first.zip");
        export(&mut a, id_a, &first);
        let manifest = super::super::read_manifest(&first, PASSWORD).unwrap();
        import(&mut b, &first, vec![ImportAction::New]).unwrap();
        let id_b = b
            .find_document_uuid(manifest.records[0].document_uuid.as_deref().unwrap())
            .unwrap()
            .unwrap();
        // 本机改了工作稿但没导出：再拿到同一份旧包，应判「本机较新」。
        update_body(&mut b, id_b, "甲改。\n");
        let again = dir.path().join("again.zip");
        export(&mut a, id_a, &again);
        let preview = inspect(
            &mut b,
            &super::super::read_manifest(&again, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::LocalAhead);
        import(&mut b, &again, vec![ImportAction::Skip]).unwrap();
        assert_eq!(b.get(id_b).unwrap().unwrap().content_markdown, "甲改。\n");
        assert_eq!(b.list(&Default::default()).unwrap().len(), 1);
    }

    #[test]
    fn same_content_with_status_change_applies_only_with_take_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲。", "状态稿");
        let first = dir.path().join("first.zip");
        export(&mut a, id_a, &first);
        import(&mut b, &first, vec![ImportAction::New]).unwrap();
        let manifest = super::super::read_manifest(&first, PASSWORD).unwrap();
        let id_b = b
            .find_document_uuid(manifest.records[0].document_uuid.as_deref().unwrap())
            .unwrap()
            .unwrap();
        b.set_status(id_b, ManuscriptStatus::Published).unwrap();
        let second = dir.path().join("second.zip");
        export(&mut b, id_b, &second);
        let second_manifest = super::super::read_manifest(&second, PASSWORD).unwrap();
        // 内容一致、只有生命周期状态不同：判定同一版本，状态是否采用由用户单独勾选。
        let preview = inspect(&mut a, &second_manifest).unwrap();
        assert_eq!(preview[0].relationship, Relationship::Same);
        import(
            &mut a,
            &second,
            vec![ImportAction::UseIncoming { take_status: true }],
        )
        .unwrap();
        let record = a.get(id_a).unwrap().unwrap();
        assert_eq!(record.status, ManuscriptStatus::Published);
        assert_eq!(record.content_markdown, "甲。");
        assert_eq!(a.get(id_a).unwrap().unwrap().pdfs.len(), 0);
    }

    #[test]
    fn archived_manuscript_only_accepts_copy_with_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲。", "归档稿");
        let first = dir.path().join("first.zip");
        export(&mut a, id_a, &first);
        import(&mut b, &first, vec![ImportAction::New]).unwrap();
        let manifest = super::super::read_manifest(&first, PASSWORD).unwrap();
        let id_b = b
            .find_document_uuid(manifest.records[0].document_uuid.as_deref().unwrap())
            .unwrap()
            .unwrap();
        a.set_status(id_a, ManuscriptStatus::Archived).unwrap();
        update_body(&mut b, id_b, "乙。");
        let second = dir.path().join("second.zip");
        export(&mut b, id_b, &second);
        let preview = inspect(
            &mut a,
            &super::super::read_manifest(&second, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::Archived);
        let summary = import(&mut a, &second, vec![ImportAction::Copy]).unwrap();
        assert_eq!(summary.created, 1);
        // 归档原稿一字不动；另存的副本是全新身份的新稿件。
        let original = a.get(id_a).unwrap().unwrap();
        assert_eq!(original.content_markdown, "甲。");
        assert_eq!(original.status, ManuscriptStatus::Archived);
        assert_eq!(a.list(&Default::default()).unwrap().len(), 2);
        let copy_id = a
            .list(&Default::default())
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .find(|id| *id != id_a)
            .unwrap();
        let copy = a.get(copy_id).unwrap().unwrap();
        assert_eq!(copy.content_markdown, "乙。");
        assert_eq!(copy.status, ManuscriptStatus::Draft);
        assert_ne!(
            a.document_identity(copy_id).unwrap().0,
            a.document_identity(id_a).unwrap().0
        );
    }

    /// 把 v2 清单降级写成 v1 旧包：不带稿件身份、版本历史与资源校验值。
    fn write_v1_zip(path: &Path, record: &super::ManifestRecord) {
        use std::io::Write;
        let mut legacy = record.clone();
        legacy.document_uuid = None;
        legacy.aliases = Vec::new();
        legacy.head_revision_uuid = None;
        legacy.revisions = Vec::new();
        for pdf in &mut legacy.pdfs {
            pdf.sha256 = None;
        }
        let manifest = super::Manifest {
            schema: 1,
            exported_at: legacy.updated_at.clone(),
            records: vec![legacy],
            assets: Vec::new(),
        };
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::write::ZipWriter::new(file);
        zip.start_file(
            super::super::MANIFEST_NAME,
            super::super::encrypted_options(PASSWORD),
        )
        .unwrap();
        zip.write_all(serde_json::to_string(&manifest).unwrap().as_bytes())
            .unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn legacy_zip_imports_once_and_links_manually_without_guessing_base() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let mut c = ManuscriptStore::open(&dir.path().join("c.db")).unwrap();
        let id_a = create(&mut a, "甲。", "旧包稿");
        let v2 = dir.path().join("v2.zip");
        export(&mut a, id_a, &v2);
        let record = super::super::read_manifest(&v2, PASSWORD).unwrap().records[0].clone();
        let v1 = dir.path().join("v1.zip");
        write_v1_zip(&v1, &record);

        // 旧包没有身份：第一次是新稿件，重复导入幂等。
        let manifest = super::super::read_manifest(&v1, PASSWORD).unwrap();
        let preview = inspect(&mut b, &manifest).unwrap();
        assert_eq!(preview[0].relationship, Relationship::Legacy);
        import(&mut b, &v1, vec![ImportAction::New]).unwrap();
        let preview =
            inspect(&mut b, &super::super::read_manifest(&v1, PASSWORD).unwrap()).unwrap();
        assert_eq!(preview[0].relationship, Relationship::Same);

        // 第三台机器上人工关联到另一篇稿件：没有内容一致的快照，不推测共同祖先。
        let id_c = create(&mut c, "丙。", "既有稿");
        let preview =
            inspect(&mut c, &super::super::read_manifest(&v1, PASSWORD).unwrap()).unwrap();
        assert_eq!(preview[0].relationship, Relationship::Legacy);
        import(
            &mut c,
            &v1,
            vec![ImportAction::LinkPending { local_id: id_c }],
        )
        .unwrap();
        assert_eq!(c.get(id_c).unwrap().unwrap().content_markdown, "丙。");
        let heads = c.pending_sync_heads(id_c).unwrap();
        assert_eq!(heads.len(), 1);
        let pending = inspect_pending(&mut c, id_c, &heads[0]).unwrap();
        assert!(pending.base.is_none(), "旧包人工关联不得推测共同祖先");
        assert!(pending.proposal.conflict_count() >= 1);
        // 逐项人工核对后确认合并：生成记录两个父版本的可见合并版本。
        let choices = vec![true; pending.proposal.conflict_count()];
        merge_pending(&mut c, id_c, &heads[0], &choices, None, false).unwrap();
        assert_eq!(c.get(id_c).unwrap().unwrap().content_markdown, "甲。");
        let (_, head) = c.document_identity(id_c).unwrap();
        let merge = c
            .sync_revisions(id_c)
            .unwrap()
            .into_iter()
            .find(|revision| Some(&revision.revision_uuid) == head.as_ref())
            .unwrap();
        assert_eq!(merge.parents.len(), 2);
        assert!(merge.visible_number.is_some());
        assert!(c.pending_sync_heads(id_c).unwrap().is_empty());
    }

    #[test]
    fn diverged_conflict_stashes_and_resumes_as_visible_two_parent_merge() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲。\n乙。\n", "冲突稿");
        let first = dir.path().join("first.zip");
        export(&mut a, id_a, &first);
        import(&mut b, &first, vec![ImportAction::New]).unwrap();
        let manifest = super::super::read_manifest(&first, PASSWORD).unwrap();
        let id_b = b
            .find_document_uuid(manifest.records[0].document_uuid.as_deref().unwrap())
            .unwrap()
            .unwrap();
        update_body(&mut a, id_a, "甲改。\n乙。\n");
        update_body(&mut b, id_b, "甲补。\n乙。\n");
        let second = dir.path().join("second.zip");
        export(&mut b, id_b, &second);
        let preview = inspect(
            &mut a,
            &super::super::read_manifest(&second, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::Diverged);
        assert!(preview[0].proposal.as_ref().unwrap().conflict_count() >= 1);
        // 暂存待处理分支：当前正文保持不变。
        let summary = import(&mut a, &second, vec![ImportAction::Pending]).unwrap();
        assert_eq!(summary.pending, 1);
        assert_eq!(
            a.get(id_a).unwrap().unwrap().content_markdown,
            "甲改。\n乙。\n"
        );
        let heads = a.pending_sync_heads(id_a).unwrap();
        assert_eq!(heads.len(), 1);
        // 从暂存恢复，逐项核对后确认：采用导入侧。
        let pending = inspect_pending(&mut a, id_a, &heads[0]).unwrap();
        let choices = vec![true; pending.proposal.conflict_count()];
        merge_pending(&mut a, id_a, &heads[0], &choices, None, false).unwrap();
        assert_eq!(
            a.get(id_a).unwrap().unwrap().content_markdown,
            "甲补。\n乙。\n"
        );
        let (_, head) = a.document_identity(id_a).unwrap();
        let merge = a
            .sync_revisions(id_a)
            .unwrap()
            .into_iter()
            .find(|revision| Some(&revision.revision_uuid) == head.as_ref())
            .unwrap();
        assert_eq!(merge.parents.len(), 2);
        assert_eq!(merge.name, "合并版本");
        assert!(merge.visible_number.is_some());
        assert_eq!(
            a.list_manuscript_versions(id_a)
                .unwrap()
                .last()
                .unwrap()
                .name,
            "合并版本"
        );
        assert!(a.pending_sync_heads(id_a).unwrap().is_empty());
    }

    struct ConfigGuard;
    impl ConfigGuard {
        fn set(dir: std::path::PathBuf) -> Self {
            crate::storage::set_test_config_dir(Some(dir));
            ConfigGuard
        }
    }
    impl Drop for ConfigGuard {
        fn drop(&mut self) {
            crate::storage::set_test_config_dir(None);
        }
    }

    #[test]
    fn image_conflicts_never_overwrite_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let image_dir = config.join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        // 与真实用户目录隔离；本测试内部两个场景顺序执行，避免并发争抢覆盖。
        let _guard = ConfigGuard::set(config.clone());
        let pic = image_dir.join("pic.png");
        std::fs::write(&pic, b"png-by-a").unwrap();

        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let id_a = create(&mut a, "![图](images/pic.png)\n内容。\n", "带图稿");
        let zip = dir.path().join("with-image.zip");
        export(&mut a, id_a, &zip);

        // 同路径不同内容：拒绝导入，且不得留下任何部分写入。
        std::fs::write(&pic, b"png-by-b-different").unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let err = import(&mut b, &zip, vec![ImportAction::New]).unwrap_err();
        assert!(
            format!("{err:#}").contains("图片同名"),
            "unexpected: {err:#}"
        );
        assert!(b.list(&Default::default()).unwrap().is_empty());
        assert_eq!(std::fs::read(&pic).unwrap(), b"png-by-b-different");

        // 同路径同内容：正常导入，本机文件不被触碰。
        std::fs::write(&pic, b"png-by-a").unwrap();
        let summary = import(&mut b, &zip, vec![ImportAction::New]).unwrap();
        assert_eq!(summary.created, 1);
        assert_eq!(std::fs::read(&pic).unwrap(), b"png-by-a");
        let id_b = b.list(&Default::default()).unwrap()[0].id;
        // 合并后的带图稿件能重新导出并往返识别为同一版本。
        let round = dir.path().join("round.zip");
        export(&mut b, id_b, &round);
        let preview = inspect(
            &mut a,
            &super::super::read_manifest(&round, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::Same);
    }

    #[test]
    fn pdfs_dedup_by_content_and_never_drop_existing() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲。", "附件稿");
        a.add_pdf(id_a, "扫描.pdf", b"one").unwrap();
        let first = dir.path().join("first.zip");
        export(&mut a, id_a, &first);
        import(&mut b, &first, vec![ImportAction::New]).unwrap();
        let manifest = super::super::read_manifest(&first, PASSWORD).unwrap();
        let id_b = b
            .find_document_uuid(manifest.records[0].document_uuid.as_deref().unwrap())
            .unwrap()
            .unwrap();
        let second = dir.path().join("second.zip");
        export(&mut b, id_b, &second);
        // 同名不同内容的附件随包带来：按内容去重、只增不删、同名自动改名。
        a.add_pdf(id_a, "扫描.pdf", b"two").unwrap();
        let third = dir.path().join("third.zip");
        export(&mut a, id_a, &third);
        let preview = inspect(
            &mut b,
            &super::super::read_manifest(&third, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::Same);
        assert!(preview[0].attachments_changed);
        let summary = import(
            &mut b,
            &third,
            vec![ImportAction::UseIncoming { take_status: false }],
        )
        .unwrap();
        assert_eq!(summary.pdfs_added, 1);
        let pdfs = b.get(id_b).unwrap().unwrap().pdfs;
        assert_eq!(pdfs.len(), 2);
        assert!(
            pdfs.iter()
                .any(|pdf| pdf.file_name == "扫描.pdf" && pdf.bytes == b"one")
        );
        assert!(
            pdfs.iter()
                .any(|pdf| pdf.file_name == "扫描.pdf（导入）" && pdf.bytes == b"two")
        );
        // 再次往返：内容一致的附件不重复添加。
        let fourth = dir.path().join("fourth.zip");
        export(&mut b, id_b, &fourth);
        let summary = import(
            &mut a,
            &fourth,
            vec![ImportAction::UseIncoming { take_status: false }],
        )
        .unwrap();
        assert_eq!(summary.pdfs_added, 0);
        assert_eq!(a.get(id_a).unwrap().unwrap().pdfs.len(), 2);
    }

    #[test]
    fn whitespace_only_local_change_merges_cleanly_and_reopens_for_export() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲 乙。\n丙。\n", "空白稿");
        let first = dir.path().join("first.zip");
        export(&mut a, id_a, &first);
        import(&mut b, &first, vec![ImportAction::New]).unwrap();
        let manifest = super::super::read_manifest(&first, PASSWORD).unwrap();
        let id_b = b
            .find_document_uuid(manifest.records[0].document_uuid.as_deref().unwrap())
            .unwrap()
            .unwrap();
        // 本机只改空白（行尾空格），导入侧改另一行正文：不重叠，应无冲突合并。
        update_body(&mut a, id_a, "甲 乙。 \n丙。\n");
        update_body(&mut b, id_b, "甲 乙。\n丙改。\n");
        let second = dir.path().join("second.zip");
        export(&mut b, id_b, &second);
        let preview = inspect(
            &mut a,
            &super::super::read_manifest(&second, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::Diverged);
        assert_eq!(preview[0].proposal.as_ref().unwrap().conflict_count(), 0);
        import(
            &mut a,
            &second,
            vec![ImportAction::Merge {
                choices: Vec::new(),
                markdown_override: None,
                take_status: false,
            }],
        )
        .unwrap();
        let merged = a.get(id_a).unwrap().unwrap().content_markdown;
        assert_eq!(merged, "甲 乙。 \n丙改。\n");
        // 合并结果照常过解析，并能重新导出、被对侧识别为可更新的后续版本。
        assert!(!crate::export::parse_markdown(&merged).is_empty());
        let third = dir.path().join("third.zip");
        export(&mut a, id_a, &third);
        let preview = inspect(
            &mut b,
            &super::super::read_manifest(&third, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::IncomingAhead);
        import(
            &mut b,
            &third,
            vec![ImportAction::UseIncoming { take_status: false }],
        )
        .unwrap();
        assert_eq!(b.get(id_b).unwrap().unwrap().content_markdown, merged);
    }

    #[test]
    fn element_changes_merge_and_conflict_through_import() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = ManuscriptStore::open(&dir.path().join("a.db")).unwrap();
        let mut b = ManuscriptStore::open(&dir.path().join("b.db")).unwrap();
        let id_a = create(&mut a, "甲。", "要素稿");
        update_snapshot(&mut a, id_a, |snapshot| {
            snapshot.profile.document_number = "1".into();
            snapshot.date = "2026-01-01".into();
        });
        let first = dir.path().join("first.zip");
        export(&mut a, id_a, &first);
        import(&mut b, &first, vec![ImportAction::New]).unwrap();
        let manifest = super::super::read_manifest(&first, PASSWORD).unwrap();
        let id_b = b
            .find_document_uuid(manifest.records[0].document_uuid.as_deref().unwrap())
            .unwrap()
            .unwrap();
        // 两侧各改一个不同要素：互不冲突，合并后都保留。
        update_snapshot(&mut a, id_a, |snapshot| {
            snapshot.profile.document_number = "2".into();
        });
        update_snapshot(&mut b, id_b, |snapshot| {
            snapshot.date = "2026-02-02".into();
        });
        let second = dir.path().join("second.zip");
        export(&mut b, id_b, &second);
        let preview = inspect(
            &mut a,
            &super::super::read_manifest(&second, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::Diverged);
        assert_eq!(preview[0].proposal.as_ref().unwrap().conflict_count(), 0);
        import(
            &mut a,
            &second,
            vec![ImportAction::Merge {
                choices: Vec::new(),
                markdown_override: None,
                take_status: false,
            }],
        )
        .unwrap();
        let snapshot = a.get(id_a).unwrap().unwrap().snapshot;
        assert_eq!(snapshot.profile.document_number, "2");
        assert_eq!(snapshot.date, "2026-02-02");

        // 两侧改同一要素为不同值：冲突，逐项核对后取导入侧。
        update_snapshot(&mut a, id_a, |snapshot| {
            snapshot.profile.document_number = "3".into();
        });
        update_snapshot(&mut b, id_b, |snapshot| {
            snapshot.profile.document_number = "9".into();
        });
        let third = dir.path().join("third.zip");
        export(&mut b, id_b, &third);
        let preview = inspect(
            &mut a,
            &super::super::read_manifest(&third, PASSWORD).unwrap(),
        )
        .unwrap();
        assert_eq!(preview[0].relationship, Relationship::Diverged);
        assert_eq!(preview[0].proposal.as_ref().unwrap().conflict_count(), 1);
        import(
            &mut a,
            &third,
            vec![ImportAction::Merge {
                choices: vec![true],
                markdown_override: None,
                take_status: false,
            }],
        )
        .unwrap();
        assert_eq!(
            a.get(id_a)
                .unwrap()
                .unwrap()
                .snapshot
                .profile
                .document_number,
            "9"
        );
    }
}
