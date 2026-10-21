//! ZIP 导入导出、分支合并落地与 PDF 附件打开/另存。
//!
//! 由 src/app/manuscript_ui.rs 拆分而来：本文件是模块 `app::manuscript_ui::sync_io`，与其它子模块共享
//! `app::manuscript_ui` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::manuscript_ui::{
    ImportPreview, PendingMergeDialog, ZipPasswordDialog, ZipPasswordPurpose,
};
use crate::app::{GongwenApp, NavPage, WorkerResult};
use crate::draft_page::DraftSession;
use crate::manuscript::{ManuscriptFilter, ManuscriptStore, ManuscriptUpdate};
use crate::manuscript_io;
use crate::models::ManuscriptStatus;
use crate::package_file;
use crate::storage;
use crate::theme;
use crate::units;
use crate::vocabulary_xlsx;
use anyhow::Context;
use std::path::PathBuf;
use std::thread;

impl GongwenApp {
    pub(crate) fn open_zip_password_dialog(&mut self, purpose: ZipPasswordPurpose) {
        self.manuscript_zip_password = Some(ZipPasswordDialog::new(
            purpose,
            self.remembered_zip_password.as_deref(),
        ));
    }

    pub(crate) fn run_zip_password_action(
        &mut self,
        purpose: ZipPasswordPurpose,
        password: String,
        remember: bool,
        include_send_package: bool,
    ) {
        let retry_import = match &purpose {
            ZipPasswordPurpose::Import(path) => Some(path.clone()),
            _ => None,
        };
        let completed = match purpose {
            ZipPasswordPurpose::FilteredExport(filter) => {
                self.perform_export_manuscripts_zip(&filter, &password, include_send_package)
            }
            ZipPasswordPurpose::SelectedExport(ids) => {
                self.perform_export_selected_manuscripts_zip(ids, &password, include_send_package)
            }
            ZipPasswordPurpose::PdfExport(options, ids) => {
                self.perform_export_selected_manuscript_pdfs(options, ids, &password)
            }
            ZipPasswordPurpose::Import(path) => self.prepare_import_manuscript(path, &password),
        };
        if completed {
            self.update_remembered_zip_password(remember.then_some(password));
        } else if let Some(path) = retry_import {
            let mut dialog = ZipPasswordDialog::new(ZipPasswordPurpose::Import(path), None);
            dialog.password = password;
            dialog.remember = remember;
            dialog.error = Some(self.status.clone());
            self.manuscript_zip_password = Some(dialog);
        }
    }

    pub(crate) fn update_remembered_zip_password(&mut self, password: Option<String>) {
        match storage::save_remembered_zip_password(password.as_deref()) {
            Ok(()) => self.remembered_zip_password = password,
            Err(error) => {
                self.status
                    .push_str(&format!(" 但未能更新记住的密码：{error:#}"));
            }
        }
    }

    /// ZIP 必须包含打开标签里的最新编辑内容，不能等两分钟自动保存。
    pub(crate) fn save_open_manuscripts_for_zip(
        &mut self,
        selected: Option<&[i64]>,
    ) -> anyhow::Result<()> {
        let updates = self
            .docs
            .iter()
            .enumerate()
            .filter_map(|(index, doc)| {
                let id = doc.manuscript_id?;
                (doc.is_dirty() && selected.is_none_or(|ids| ids.contains(&id)))
                    .then(|| (index, id, doc.draft.clone(), doc.generated_markdown.clone()))
            })
            .collect::<Vec<_>>();
        for (index, id, snapshot, content_markdown) in updates {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            let record = store
                .get(id)?
                .ok_or_else(|| anyhow::anyhow!("正在编辑的稿件不存在"))?;
            if record.status != ManuscriptStatus::Draft {
                anyhow::bail!("稿件《{}》不是草稿，不能同步未保存修改", record.title);
            }
            store.update(
                id,
                &ManuscriptUpdate {
                    snapshot,
                    content_markdown,
                    notes: record.notes,
                },
            )?;
            self.docs[index].mark_saved();
            self.manuscript_dirty = true;
        }
        Ok(())
    }

    pub(crate) fn export_manuscripts_zip(&mut self) {
        if self.manuscript_store.is_none() {
            self.status = "稿件库不可用，无法导出。".into();
            return;
        }
        let filter = self.manuscript_filter.clone();
        self.open_zip_password_dialog(ZipPasswordPurpose::FilteredExport(filter));
    }

    pub(crate) fn perform_export_manuscripts_zip(
        &mut self,
        filter: &ManuscriptFilter,
        password: &str,
        include_send_package: bool,
    ) -> bool {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let default_name = format!("公文稿件-{stamp}.{}", package_file::EXTENSION);
        let Some(path) = save_package_dialog(&default_name) else {
            return false;
        };
        if let Err(error) = self.save_open_manuscripts_for_zip(None) {
            self.status = format!("导出前保存当前工作稿失败：{error:#}");
            return false;
        }
        let result: anyhow::Result<manuscript_io::ExportSummary> =
            match self.manuscript_store.as_mut() {
                Some(store) => manuscript_io::export_zip(
                    store,
                    filter,
                    &self.config.vocabulary,
                    &path,
                    password,
                    include_send_package,
                ),
                None => Err(anyhow::anyhow!("稿件库不可用")),
            };
        match result {
            Ok(summary) => {
                self.status = format!(
                    "已导出 {} 篇稿件、{} 个 PDF 附件到 {}。",
                    summary.records,
                    summary.pdfs,
                    path.display()
                );
                true
            }
            Err(error) => {
                self.status = format!("导出失败：{error:#}");
                false
            }
        }
    }

    pub(crate) fn export_selected_manuscripts_zip(&mut self) {
        if self.manuscript_selected.is_empty() {
            self.status = "请先勾选要导出的稿件。".into();
            return;
        }
        if self.manuscript_store.is_none() {
            self.status = "稿件库不可用，无法导出。".into();
            return;
        }
        let ids = self.manuscript_selected.iter().copied().collect();
        self.open_zip_password_dialog(ZipPasswordPurpose::SelectedExport(ids));
    }

    pub(crate) fn perform_export_selected_manuscripts_zip(
        &mut self,
        mut ids: Vec<i64>,
        password: &str,
        include_send_package: bool,
    ) -> bool {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let default_name = format!("所选公文稿件-{stamp}.{}", package_file::EXTENSION);
        let Some(path) = save_package_dialog(&default_name) else {
            return false;
        };
        // 随行件也要进包时，它们在起草页里开着的工作稿同样先存一下。
        if include_send_package
            && let Some(store) = self.manuscript_store.as_ref()
            && let Ok(expanded) = store.with_send_package_items(&ids)
        {
            ids = expanded;
        }
        if let Err(error) = self.save_open_manuscripts_for_zip(Some(&ids)) {
            self.status = format!("导出前保存所选工作稿失败：{error:#}");
            return false;
        }
        let result = match self.manuscript_store.as_mut() {
            Some(store) => manuscript_io::export_zip_selected(
                store,
                &ids,
                &self.config.vocabulary,
                &path,
                password,
                false,
            ),
            None => Err(anyhow::anyhow!("稿件库不可用")),
        };
        match result {
            Ok(summary) => {
                self.status = format!(
                    "已导出所选 {} 篇稿件、{} 个 PDF 附件到 {}。",
                    summary.records,
                    summary.pdfs,
                    path.display()
                );
                true
            }
            Err(error) => {
                self.status = format!("导出所选稿件失败：{error:#}");
                false
            }
        }
    }

    /// 把勾选的稿件按选项导出为 PDF 集合并打包 zip，后台线程执行避免卡界面。
    /// 盖章件直接取附件；非盖章件编译 TeX 生成，缺引擎或失败时该篇记入汇总。
    pub(crate) fn perform_export_selected_manuscript_pdfs(
        &mut self,
        options: manuscript_io::PdfExportOptions,
        ids: Vec<i64>,
        password: &str,
    ) -> bool {
        if ids.is_empty() {
            self.status = "请先勾选要导出的稿件。".into();
            return false;
        }
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            self.status = "稿件库路径不可用，无法导出 PDF。".into();
            return false;
        };
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let default_name = format!("所选公文PDF-{stamp}.zip");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP PDF 压缩包", &["zip"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return false;
        };
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
        let numbering = self.config.numbering;
        let password = password.to_string();
        self.manuscript_pdf_export_busy = true;
        self.status = format!("正在导出 {} 篇稿件的 PDF…", ids.len());
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result: anyhow::Result<manuscript_io::PdfExportSummary> = (|| {
                let mut store = ManuscriptStore::open(&db_path)
                    .with_context(|| format!("无法打开稿件库 {}", db_path.display()))?;
                manuscript_io::export_selected_pdfs(
                    &mut store,
                    &ids,
                    &options,
                    &vocabulary,
                    &fonts,
                    &numbering,
                    &path,
                    &password,
                    |_| {},
                )
            })();
            let _ = tx.send(WorkerResult::ManuscriptPdfExport {
                path,
                result: result.map_err(|error| format!("{error:#}")),
            });
        });
        true
    }

    pub(crate) fn start_import_manuscript(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(package_file::FILTER_NAME, package_file::IMPORT_EXTENSIONS)
            .pick_file()
        else {
            return;
        };
        self.receive_import_package(path);
    }

    /// 收到一个要导入的稿件包：按钮选的、双击启动带进来的、拖进窗口的都走这里。
    /// 先复制到暂存目录再排队，真正弹口令框在 [`Self::pump_pending_imports`]。
    pub(crate) fn receive_import_package(&mut self, path: PathBuf) {
        if !package_file::is_package_path(&path) {
            self.status = format!("{} 不是公文稿件包（.gwx / .zip）。", path.display());
            return;
        }
        let staged =
            package_file::staging_root().and_then(|root| package_file::stage(&path, &root));
        match staged {
            Ok(staged) => {
                self.pending_imports.push_back(staged);
                if self.import_dialog_open() {
                    self.status = format!(
                        "已收到稿件包，当前导入处理完后接着导入（还有 {} 个等待）。",
                        self.pending_imports.len()
                    );
                }
            }
            Err(error) => self.status = format!("无法导入稿件包：{error:#}"),
        }
    }

    /// 启动时调用一次：清掉上次留下的暂存副本，再收下命令行带进来的稿件包
    /// （双击 `.gwx` 启动时系统把路径作为参数传进来）。
    pub(crate) fn receive_startup_imports(&mut self, paths: Vec<PathBuf>) {
        if let Ok(root) = package_file::staging_root() {
            package_file::clear_staging(&root);
        }
        for path in paths {
            self.receive_import_package(path);
        }
    }

    /// 每帧调用：把拖进窗口的稿件包收进导入队列；拖着稿件包悬停在窗口上时盖一层提示。
    /// 别的文件类型不处理（目前窗口没有别的拖放入口）。
    pub(crate) fn receive_dropped_packages(&mut self, ctx: &egui::Context) {
        let (hovering, dropped) = ctx.input(|input| {
            let hovering = input.raw.hovered_files.iter().any(|file| {
                file.path
                    .as_deref()
                    .is_some_and(package_file::is_package_path)
            });
            let dropped = input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path())
                .filter(|path| package_file::is_package_path(path))
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            (hovering, dropped)
        });
        if hovering {
            paint_drop_hint(ctx);
        }
        for path in dropped {
            self.receive_import_package(path);
        }
    }

    /// 口令框、导入预览或待处理分支合并框是否开着。开着时新来的包先排队。
    fn import_dialog_open(&self) -> bool {
        self.manuscript_zip_password.is_some()
            || self.manuscript_import_preview.is_some()
            || self.pending_merge.is_some()
    }

    /// 每帧调用：上一个包处理完（确认、取消或出错关框）就删掉它的副本，
    /// 再把队列里的下一个切到稿件管理页、弹出口令框。
    pub(crate) fn pump_pending_imports(&mut self) {
        if self.import_dialog_open() {
            return;
        }
        if let Some(done) = self.active_import.take()
            && let Ok(root) = package_file::staging_root()
        {
            package_file::discard(&done, &root);
        }
        let Some(next) = self.pending_imports.pop_front() else {
            return;
        };
        self.open_page(NavPage::Manuscript);
        self.active_import = Some(next.clone());
        self.open_zip_password_dialog(ZipPasswordPurpose::Import(next));
        if !self.pending_imports.is_empty() {
            self.status = format!(
                "这个稿件包处理完后还有 {} 个等待导入。",
                self.pending_imports.len()
            );
        }
    }

    pub(crate) fn prepare_import_manuscript(&mut self, path: PathBuf, password: &str) -> bool {
        let result: anyhow::Result<ImportPreview> = (|| {
            let manifest = manuscript_io::read_manifest(&path, password)?;
            let vocabulary = match manuscript_io::read_vocabulary(&path, password) {
                Ok(vocabulary) => vocabulary,
                Err(error) => {
                    self.status = format!("稿件包词库读取失败（不影响稿件导入）：{error:#}");
                    None
                }
            };
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            let relations = manuscript_io::sync::inspect(store, &manifest)?;
            use manuscript_io::sync::{ImportAction, Relationship, default_take_package};
            let package_diffs = relations
                .iter()
                .zip(&manifest.records)
                .map(|(relation, record)| {
                    manuscript_io::sync::package_difference(store, relation.local_id, record)
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let actions = relations
                .iter()
                .enumerate()
                .map(|(index, relation)| {
                    let take_package =
                        default_take_package(relation.relationship, package_diffs[index]);
                    match relation.relationship {
                        Relationship::New | Relationship::Legacy
                            if relation.candidates.is_empty() =>
                        {
                            ImportAction::New
                        }
                        Relationship::New | Relationship::Legacy => ImportAction::Skip,
                        Relationship::IncomingAhead | Relationship::Equivalent => {
                            ImportAction::UseIncoming {
                                take_status: false,
                                take_package,
                            }
                        }
                        // 内容一样但清单不同、且本机清单为空：采用导入版不会丢任何东西。
                        Relationship::Same
                            if relation.attachments_changed
                                || take_package
                                || relation.local_status.is_some_and(|status| {
                                    status != manifest.records[index].status
                                }) =>
                        {
                            ImportAction::UseIncoming {
                                take_status: relation
                                    .local_status
                                    .is_some_and(|status| status != manifest.records[index].status),
                                take_package,
                            }
                        }
                        Relationship::Diverged
                            if relation
                                .proposal
                                .as_ref()
                                .is_some_and(|proposal| proposal.conflict_count() == 0) =>
                        {
                            ImportAction::Merge {
                                choices: Vec::new(),
                                markdown_override: None,
                                take_status: false,
                                take_package,
                            }
                        }
                        Relationship::Diverged | Relationship::NoBase => ImportAction::Pending,
                        Relationship::Archived | Relationship::Same | Relationship::LocalAhead => {
                            ImportAction::Skip
                        }
                    }
                })
                .collect();
            let manifest_hash =
                crate::manuscript::sync::bytes_hash(&serde_json::to_vec(&manifest)?);
            let selected = vec![true; manifest.records.len()];
            let reviews = (0..selected.len()).map(|_| None).collect();
            Ok(ImportPreview {
                manifest,
                zip_path: path,
                selected,
                keyword: String::new(),
                relations,
                actions,
                reviews,
                manifest_hash,
                package_diffs,
                focused: None,
                vocabulary,
                merge_vocabulary: true,
                password: password.to_string(),
                notice: None,
            })
        })();
        match result {
            Ok(preview) => {
                self.manuscript_import_preview = Some(preview);
                self.status = "已读取稿件包，请逐篇核对处理方式后确认。".into();
                true
            }
            Err(error) => {
                self.status = format!("读取稿件包失败：{error:#}");
                false
            }
        }
    }

    pub(crate) fn confirm_import(&mut self) {
        use manuscript_io::sync::ImportAction;
        let Some(mut preview) = self.manuscript_import_preview.take() else {
            return;
        };
        let actions = preview
            .actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                if preview.selected[index] {
                    action.clone()
                } else {
                    ImportAction::Skip
                }
            })
            .collect::<Vec<_>>();
        let unreviewed = actions.iter().enumerate().find_map(|(index, action)| {
            let ImportAction::Merge {
                choices,
                markdown_override,
                ..
            } = action
            else {
                return None;
            };
            let proposal = preview.relations[index].proposal.as_ref()?;
            let (snapshot, merged, _) = proposal.resolve(choices).ok()?;
            let candidate = markdown_override.as_deref().unwrap_or(&merged);
            let hash = crate::manuscript::sync::payload_hash(&snapshot, candidate, "").ok()?;
            if preview.reviews[index]
                .as_ref()
                .is_some_and(|review| review.hash == hash)
            {
                None
            } else {
                Some(index)
            }
        });
        if let Some(index) = unreviewed {
            preview.focused = Some(index);
            self.status = "请先逐篇查看合并候选和规则复核结果，再确认导入。".into();
            preview.notice = Some(self.status.clone());
            self.manuscript_import_preview = Some(preview);
            return;
        }
        for (action, relation) in actions.iter().zip(&preview.relations) {
            if matches!(
                action,
                ImportAction::UseIncoming { .. } | ImportAction::Merge { .. }
            ) && let Some(id) = relation.local_id
                && self
                    .docs
                    .iter()
                    .any(|doc| doc.manuscript_id == Some(id) && doc.is_dirty())
            {
                self.status = "这篇稿件在起草页有未保存修改，请先保存后再导入。".into();
                preview.notice = Some(self.status.clone());
                self.manuscript_import_preview = Some(preview);
                return;
            }
        }
        let result = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            manuscript_io::sync::import_with_actions(
                store,
                &preview.zip_path,
                &preview.password,
                &preview.manifest_hash,
                &actions,
                &preview.relations,
            )
        })();
        match result {
            Ok(summary) => {
                self.manuscript_dirty = true;
                for id in &summary.changed_ids {
                    self.refresh_open_manuscript_after_sync(*id);
                    if self
                        .manuscript_detail
                        .as_ref()
                        .is_some_and(|detail| detail.id == *id)
                    {
                        self.refresh_detail(*id);
                    }
                }
                let mut message = format!(
                    "已新建 {} 篇、更新 {} 篇、合并 {} 篇；暂存 {} 条分支，跳过 {} 篇，新增 {} 个 PDF 附件。",
                    summary.created,
                    summary.updated,
                    summary.merged,
                    summary.pending,
                    summary.skipped,
                    summary.pdfs_added
                );
                if preview.merge_vocabulary
                    && let Some(vocabulary) = &preview.vocabulary
                {
                    let report = vocabulary_xlsx::merge(
                        &mut self.config.vocabulary,
                        vocabulary.entries.clone(),
                    );
                    units::normalize(&mut self.config.vocabulary);
                    match storage::save(&self.config) {
                        Ok(()) => message.push_str(&format!(
                            " 词库合并：新增 {}、更新 {}、不变 {} 条。",
                            report.added, report.updated, report.unchanged
                        )),
                        Err(error) => message.push_str(&format!(
                            " 词库已合并但保存失败：{error:#}（请检查配置目录权限）"
                        )),
                    }
                }
                self.status = message;
            }
            Err(error) => {
                self.status = format!("导入失败：{error:#}");
                preview.notice = Some(self.status.clone());
                self.manuscript_import_preview = Some(preview);
            }
        }
    }

    pub(crate) fn open_pending_merge(&mut self, id: i64, head: &str) {
        let result = self
            .manuscript_store
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))
            .and_then(|store| manuscript_io::sync::inspect_pending(store, id, head));
        match result {
            Ok(preview) => {
                let choices = vec![false; preview.proposal.conflict_count()];
                self.pending_merge = Some(PendingMergeDialog {
                    manuscript_id: id,
                    preview,
                    choices,
                    markdown_override: None,
                    take_status: false,
                    review: None,
                    notice: None,
                });
                self.status = "已打开待处理分支的合并预览。".into();
            }
            Err(error) => self.status = format!("无法打开待处理分支：{error:#}"),
        }
    }

    pub(crate) fn confirm_pending_merge(&mut self) {
        let Some(mut dialog) = self.pending_merge.take() else {
            return;
        };
        let reviewed = dialog
            .preview
            .proposal
            .resolve(&dialog.choices)
            .ok()
            .and_then(|(snapshot, merged, _)| {
                let candidate = dialog.markdown_override.as_deref().unwrap_or(&merged);
                crate::manuscript::sync::payload_hash(&snapshot, candidate, "").ok()
            })
            .is_some_and(|hash| {
                dialog
                    .review
                    .as_ref()
                    .is_some_and(|review| review.hash == hash)
            });
        if !reviewed {
            self.status = "合并候选已变化，请先查看最新复核结果。".into();
            dialog.notice = Some(self.status.clone());
            self.pending_merge = Some(dialog);
            return;
        }
        let id = dialog.manuscript_id;
        if self
            .docs
            .iter()
            .any(|doc| doc.manuscript_id == Some(id) && doc.is_dirty())
        {
            self.status = "稿件在起草页有未保存修改，请先保存后再合并。".into();
            dialog.notice = Some(self.status.clone());
            self.pending_merge = Some(dialog);
            return;
        }
        let result = self
            .manuscript_store
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))
            .and_then(|store| {
                manuscript_io::sync::merge_pending(
                    store,
                    id,
                    &dialog.preview,
                    &dialog.choices,
                    dialog.markdown_override.as_deref(),
                    dialog.take_status,
                )
            });
        match result {
            Ok(()) => {
                self.manuscript_dirty = true;
                self.refresh_open_manuscript_after_sync(id);
                if self
                    .manuscript_detail
                    .as_ref()
                    .is_some_and(|detail| detail.id == id)
                {
                    self.refresh_detail(id);
                }
                self.status = "待处理分支已合并为新的可见版本。".into();
            }
            Err(error) => {
                self.status = format!("合并待处理分支失败：{error:#}");
                dialog.notice = Some(self.status.clone());
                self.pending_merge = Some(dialog);
            }
        }
    }

    pub(crate) fn refresh_open_manuscript_after_sync(&mut self, id: i64) {
        let record = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.get(id).ok().flatten());
        let Some(record) = record else {
            return;
        };
        for doc in &mut self.docs {
            if doc.manuscript_id == Some(id) {
                let key = doc.key;
                let mut session = DraftSession::from_parts(
                    key,
                    Some(id),
                    record.snapshot.clone(),
                    record.content_markdown.clone(),
                );
                session.record_status = record.status;
                session.mark_saved();
                *doc = session;
            }
        }
    }

    pub(crate) fn open_pdf_attachment(&mut self, id: i64) {
        let (path, title) = {
            let detail = self.manuscript_detail.as_ref();
            let Some(detail) = detail else { return };
            let Some(pdf) = detail.pdfs.iter().find(|p| p.id == id) else {
                return;
            };
            let path = self.temp_pdf_path(detail.id, id);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(error) = std::fs::write(&path, &pdf.bytes) {
                self.status = format!("写入临时 PDF 失败：{error}");
                return;
            }
            (path, pdf.file_name.clone())
        };
        self.open_pdf(path, Some(title));
    }

    pub(crate) fn save_pdf_attachment(&mut self, id: i64) {
        let Some((file_name, bytes)) = self.manuscript_detail.as_ref().and_then(|d| {
            d.pdfs
                .iter()
                .find(|p| p.id == id)
                .map(|p| (p.file_name.clone(), p.bytes.clone()))
        }) else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(&file_name)
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, &bytes) {
            Ok(()) => self.status = format!("已保存附件到 {}。", path.display()),
            Err(error) => self.status = format!("保存附件失败：{error}"),
        }
    }

    pub(crate) fn temp_pdf_path(&self, manuscript_id: i64, attachment_id: i64) -> PathBuf {
        PathBuf::from(&self.config.output_dir)
            .join("temp")
            .join(format!("manuscript_{manuscript_id}_{attachment_id}.pdf"))
    }
}

/// 拖着稿件包悬停在窗口上时的整窗提示：半透明遮罩 + 居中一句话。
fn paint_drop_hint(ctx: &egui::Context) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("gw-package-drop-hint"),
    ));
    let screen = ctx.content_rect();
    painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(96));
    let text = painter.layout_no_wrap(
        "松开即导入稿件包".to_owned(),
        egui::FontId::proportional(18.0),
        theme::text(),
    );
    let card = egui::Rect::from_center_size(screen.center(), text.size() + egui::vec2(48.0, 28.0));
    painter.rect_filled(card, 8.0, theme::surface());
    painter.rect_stroke(
        card,
        8.0,
        egui::Stroke::new(1.5, theme::accent()),
        egui::StrokeKind::Inside,
    );
    painter.galley(card.center() - text.size() / 2.0, text, theme::text());
}

/// 稿件包的保存框。过滤器只列 `.gwx`；用户删掉后缀时补上，明确写 `.zip` 的保留
/// （发给还没升级的旧版程序时用）。
fn save_package_dialog(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(package_file::FILTER_NAME, &[package_file::EXTENSION])
        .set_file_name(default_name)
        .save_file()
        .map(package_file::with_package_extension)
}
