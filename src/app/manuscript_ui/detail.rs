//! 稿件详情页：元数据、版本、操作按钮与状态流转。
//!
//! 由 src/app/manuscript_ui.rs 拆分而来：本文件是模块 `app::manuscript_ui::detail`，与其它子模块共享
//! `app::manuscript_ui` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::manuscript_ui::{ArchivePending, BatchDeleteConfirm, ManuscriptAction, PdfAction};
use crate::app::{
    GongwenApp, VersionDiffState, VersionScope, joined_metadata, metadata_grid_row,
    present_or_dash, short_date, status_color, summarize,
};
use crate::doc_import;
use crate::draft_page::DraftSession;
use crate::export;
use crate::manuscript::send_package::{PinChoice, is_owner_kind};
use crate::manuscript::{ManuscriptRecord, ManuscriptUpdate, NewManuscript};
use crate::manuscript_io;
use crate::models::{ManuscriptStatus, TemplateKind};
use crate::theme;
use eframe::egui;
use std::path::PathBuf;

impl GongwenApp {
    /// 稿件列表右侧的资料卡。列表行只负责切换这里的选中项；正文查看和编辑
    /// 始终打开独立的公文标签。返回本帧要执行的 PDF 附件操作。
    pub(crate) fn manuscript_detail_ui(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<ManuscriptAction>,
        single_page: bool,
    ) -> Option<PdfAction> {
        let Some(detail_id) = self.manuscript_detail.as_ref().map(|d| d.id) else {
            ui.heading("稿件资料");
            ui.separator();
            ui.add_space(8.0);
            ui.weak("单击左侧任意一行，在这里查看该稿件的元数据、版本和附件。");
            ui.add_space(8.0);
            ui.weak("双击一行或点“查看详情”，可直接打开公文界面。");
            return None;
        };
        let mut clear_selection = false;
        let mut open = false;
        let mut create_from_existing = false;
        let mut pdf_action: Option<PdfAction> = None;
        let mut delete_pdf: Option<i64> = None;
        let mut add_pdfs: Vec<PathBuf> = Vec::new();
        let pending_heads = self
            .manuscript_store
            .as_ref()
            .and_then(|store| store.pending_sync_heads(detail_id).ok())
            .unwrap_or_default();
        let mut pending_to_open: Option<String> = None;

        let detail = self.manuscript_detail.as_ref().unwrap();
        ui.horizontal(|ui| {
            if single_page && ui.button("← 返回列表").clicked() {
                clear_selection = true;
            }
            ui.heading("稿件资料");
            ui.colored_label(status_color(detail.status), detail.status.label());
            if !single_page {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, theme::Icon::X, "清除选择").clicked() {
                        clear_selection = true;
                    }
                });
            }
        });
        ui.separator();
        let open_label = if matches!(
            detail.status,
            ManuscriptStatus::Published | ManuscriptStatus::Archived
        ) {
            "打开查看"
        } else {
            "打开编辑"
        };
        let open_clicked = theme::accent_scope(ui, |ui| {
            ui.add_sized(
                [ui.available_width(), 30.0],
                theme::primary_button_widget(theme::Icon::PencilLine, open_label),
            )
        })
        .on_hover_text("在独立的公文标签中打开；发布和归档稿自动只读")
        .clicked();
        if open_clicked {
            open = true;
        }
        if ui
            .add_sized(
                [ui.available_width(), 28.0],
                theme::icon_text_button(theme::Icon::Copy, "基于此公文新建"),
            )
            .on_hover_text("复制行文要素和正文，新稿不会继承状态、版本或 PDF 附件")
            .clicked()
        {
            create_from_existing = true;
        }
        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .id_salt("manuscript_metadata_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.strong("公文元数据");
                ui.add_space(4.0);
                egui::Grid::new("manuscript_metadata_grid")
                    .num_columns(2)
                    .min_col_width(76.0)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        let profile = &detail.snapshot.profile;
                        metadata_grid_row(ui, "标题", &detail.title);
                        metadata_grid_row(ui, "文档类型", detail.kind.label());
                        // 密级按文档类型取：研究报告的在封面元数据里，不在版式要素里。
                        let (level, period) = detail.snapshot.security_marking();
                        metadata_grid_row(ui, "密级", joined_metadata(&[level, period]).as_str());
                        metadata_grid_row(ui, "文号", present_or_dash(&detail.doc_number));
                        if detail.kind.has_document_number() {
                            metadata_grid_row(
                                ui,
                                "发文年份",
                                present_or_dash(&detail.snapshot.document_year()),
                            );
                        }
                        metadata_grid_row(ui, "成文日期", present_or_dash(&detail.doc_date));
                        metadata_grid_row(ui, "发文单位", present_or_dash(&profile.issuing_unit));
                        metadata_grid_row(ui, "主送", present_or_dash(&profile.recipient));
                        metadata_grid_row(ui, "抄送", present_or_dash(&profile.copies_to));
                        if detail.kind.is_approval() {
                            metadata_grid_row(
                                ui,
                                "呈报领导",
                                present_or_dash(&profile.reporting_leaders),
                            );
                            metadata_grid_row(
                                ui,
                                "落款单位",
                                present_or_dash(&profile.signing_unit),
                            );
                        }
                        metadata_grid_row(
                            ui,
                            "承办单位",
                            present_or_dash(if detail.kind == TemplateKind::RedHeadApproval {
                                &profile.joint_responsible_units
                            } else {
                                &profile.responsible_unit
                            }),
                        );
                        let contacts_metadata = if detail.kind == TemplateKind::RedHeadApproval {
                            profile
                                .joint_contacts
                                .iter()
                                .map(|contact| format!("{} {}", contact.name, contact.phone))
                                .collect::<Vec<_>>()
                                .join("；")
                        } else {
                            joined_metadata(&[
                                profile.contact_person.as_str(),
                                profile.contact_phone.as_str(),
                            ])
                        };
                        metadata_grid_row(ui, "联系人", present_or_dash(&contacts_metadata));
                        if !detail.snapshot.meeting_time.trim().is_empty() {
                            metadata_grid_row(ui, "会议时间", &detail.snapshot.meeting_time);
                        }
                        if !profile.meeting_location.trim().is_empty() {
                            metadata_grid_row(ui, "会议地点", &profile.meeting_location);
                        }
                        if !detail.snapshot.attendees.trim().is_empty() {
                            metadata_grid_row(ui, "参加人员", &detail.snapshot.attendees);
                        }
                        metadata_grid_row(ui, "创建", &short_date(&detail.created_at));
                        metadata_grid_row(ui, "更新", &short_date(&detail.updated_at));
                        if let Some(at) = &detail.published_at {
                            metadata_grid_row(ui, "发布", &short_date(at));
                        }
                        if let Some(at) = &detail.archived_at {
                            metadata_grid_row(ui, "归档", &short_date(at));
                        }
                        if !detail.notes.trim().is_empty() {
                            metadata_grid_row(ui, "备注", &detail.notes);
                        }
                    });

                if is_owner_kind(detail.kind) {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(format!(
                            "送批材料（{}）",
                            self.manuscript_package_titles.len()
                        ));
                        if ui
                            .add(theme::icon_text_button(theme::Icon::Package, "管理…"))
                            .on_hover_text("查看各件的提交版本，增删、调整顺序")
                            .clicked()
                        {
                            *action = Some(ManuscriptAction::OpenSendPackage(detail_id));
                        }
                    });
                    if self.manuscript_package_titles.is_empty() {
                        ui.weak("还没有送批材料。");
                    }
                    for (index, title) in self.manuscript_package_titles.iter().enumerate() {
                        ui.add(egui::Label::new(format!("{}. {title}", index + 1)).truncate())
                            .on_hover_text(title);
                    }
                }
                if !self.manuscript_referrers.is_empty() {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.strong("用作以下呈批件的送批材料");
                    for owner in &self.manuscript_referrers {
                        ui.horizontal(|ui| {
                            ui.colored_label(status_color(owner.status), owner.status.label());
                            if ui
                                .add(egui::Link::new(summarize(&owner.title, 24)))
                                .on_hover_text(format!("{}\n打开它的送批材料", owner.title))
                                .clicked()
                            {
                                *action = Some(ManuscriptAction::OpenSendPackage(owner.owner_id));
                            }
                        });
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("版本历史（{}）", self.manuscript_versions.len()));
                    if !self.manuscript_versions.is_empty()
                        && ui
                            .add(theme::icon_text_button(theme::Icon::Compare, "对照…"))
                            .on_hover_text("默认对照最新版与上一版")
                            .clicked()
                    {
                        *action = Some(ManuscriptAction::OpenVersionDiff {
                            manuscript_id: detail_id,
                        });
                    }
                });
                if self.manuscript_versions.is_empty() {
                    ui.weak("暂无已提交版本。");
                } else {
                    for version in self.manuscript_versions.iter().rev() {
                        theme::card().show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(format!("v{}", version.version_number));
                                ui.label(&version.name);
                                if version.is_latest {
                                    theme::chip(
                                        ui,
                                        "最新",
                                        theme::success(),
                                        theme::success_soft(),
                                    );
                                }
                                ui.weak(short_date(&version.created_at));
                            });
                            if !version.comment.trim().is_empty() {
                                ui.weak(summarize(&version.comment, 50));
                            }
                            ui.horizontal_wrapped(|ui| {
                                if detail.status != ManuscriptStatus::Archived
                                    && ui.small_button("载入编辑").clicked()
                                {
                                    *action = Some(ManuscriptAction::LoadVersion {
                                        manuscript_id: detail_id,
                                        version_number: version.version_number,
                                    });
                                }
                                if detail.status != ManuscriptStatus::Archived
                                    && ui.small_button("回退至此版").clicked()
                                {
                                    *action = Some(ManuscriptAction::RevertPending {
                                        manuscript_id: detail_id,
                                        version_number: version.version_number,
                                    });
                                }
                                if ui.small_button("与上一版对照").clicked() {
                                    *action = Some(ManuscriptAction::DiffVersion {
                                        manuscript_id: detail_id,
                                        version_number: version.version_number,
                                    });
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }
                }

                if !pending_heads.is_empty() {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.strong(format!("待处理导入分支（{}）", pending_heads.len()));
                    for head in &pending_heads {
                        ui.horizontal(|ui| {
                            ui.weak(format!("版本 {}", &head[..8.min(head.len())]));
                            if detail.status != ManuscriptStatus::Archived
                                && ui.small_button("继续合并").clicked()
                            {
                                pending_to_open = Some(head.clone());
                            }
                        });
                    }
                }

                ui.add_space(6.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("PDF 附件（{}）", detail.pdfs.len()));
                    if ui
                        .add(theme::icon_text_button(theme::Icon::Paperclip, "添加…"))
                        .on_hover_text("导入扫描盖章件等；归档后也可补充")
                        .clicked()
                        && let Some(paths) = rfd::FileDialog::new()
                            .add_filter("扫描盖章 PDF", &["pdf"])
                            .pick_files()
                    {
                        add_pdfs.extend(paths);
                    }
                });
                if detail.pdfs.is_empty() {
                    ui.weak("暂无附件。");
                }
                for pdf in &detail.pdfs {
                    theme::card().show(ui, |ui| {
                        ui.add(egui::Label::new(&pdf.file_name).truncate())
                            .on_hover_text(&pdf.file_name);
                        ui.weak(short_date(&pdf.added_at));
                        ui.horizontal_wrapped(|ui| {
                            if ui.small_button("打开").clicked() {
                                pdf_action = Some(PdfAction::Open(pdf.id));
                            }
                            if ui.small_button("另存为").clicked() {
                                pdf_action = Some(PdfAction::SaveAs(pdf.id));
                            }
                            if self.manuscript_detail_delete_pdf == Some(pdf.id) {
                                if ui.small_button("确认删除").clicked() {
                                    delete_pdf = Some(pdf.id);
                                }
                                if ui.small_button("取消").clicked() {
                                    self.manuscript_detail_delete_pdf = None;
                                }
                            } else if ui.small_button("删除").clicked() {
                                self.manuscript_detail_delete_pdf = Some(pdf.id);
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });

        if let Some(head) = pending_to_open {
            self.open_pending_merge(detail_id, &head);
        }
        if clear_selection {
            self.manuscript_detail = None;
            self.manuscript_detail_delete_pdf = None;
            self.manuscript_versions.clear();
        } else if create_from_existing {
            *action = Some(ManuscriptAction::CreateFromExisting(detail_id));
        } else if open {
            *action = Some(ManuscriptAction::Edit(detail_id));
        }
        if let Some(pdf_id) = delete_pdf {
            self.manuscript_detail_delete_pdf = None;
            pdf_action = Some(PdfAction::Delete(pdf_id));
        }
        if !add_pdfs.is_empty() {
            let result: anyhow::Result<usize> = (|| {
                let store = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                let mut added = 0;
                for path in &add_pdfs {
                    let bytes = std::fs::read(path)?;
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("附件.pdf")
                        .to_string();
                    store.add_pdf(detail_id, &name, &bytes)?;
                    added += 1;
                }
                Ok(added)
            })();
            self.reload_detail();
            match result {
                Ok(added) => self.status = format!("已添加 {added} 个附件。"),
                Err(error) => self.status = format!("添加附件失败：{error:#}"),
            }
        }
        pdf_action
    }

    pub(crate) fn apply_manuscript_action(&mut self, action: ManuscriptAction) {
        match action {
            ManuscriptAction::Detail(id) => self.refresh_detail(id),
            ManuscriptAction::Edit(id) => self.open_in_editor(id),
            ManuscriptAction::CreateFromExisting(id) => self.create_from_existing(id),
            ManuscriptAction::Publish(id) => {
                self.transition_status(id, ManuscriptStatus::Published);
                self.sync_record_status(id);
            }
            ManuscriptAction::RevertToDraft(id) => {
                self.transition_status(id, ManuscriptStatus::Draft);
                self.sync_record_status(id);
            }
            ManuscriptAction::DeletePending(id) => {
                self.manuscript_delete_refs = self
                    .manuscript_store
                    .as_ref()
                    .and_then(|store| store.send_package_referrers(id).ok())
                    .unwrap_or_default();
                self.manuscript_delete_confirm = Some(id);
            }
            ManuscriptAction::OpenSendPackage(id) => self.open_send_package(id),
            ManuscriptAction::ArchivePending(id) => {
                let send_package = self
                    .manuscript_store
                    .as_ref()
                    .and_then(|store| store.archive_pin_preview(id).ok())
                    .flatten();
                self.manuscript_archive_pending = Some(ArchivePending {
                    manuscript_id: id,
                    pdf_paths: Vec::new(),
                    send_package,
                    pin_choice: PinChoice::default(),
                });
            }
            ManuscriptAction::Archive(id) => {
                let (pdfs, pin_choice) = self
                    .manuscript_archive_pending
                    .take()
                    .map(|p| (p.pdf_paths, p.pin_choice))
                    .unwrap_or_default();
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    // 挂着送批材料时连同钉版在一个事务里完成；没挂的就是普通归档。
                    store.archive_with_send_package(id, pin_choice)?;
                    for path in &pdfs {
                        let bytes = std::fs::read(path)?;
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("扫描件.pdf")
                            .to_string();
                        store.add_pdf(id, &name, &bytes)?;
                    }
                    Ok(())
                })();
                self.manuscript_dirty = true;
                match result {
                    Ok(()) => {
                        self.sync_record_status(id);
                        self.status = format!("已归档，附带 {} 个 PDF 附件。", pdfs.len());
                    }
                    Err(error) => self.status = format!("归档失败：{error:#}"),
                }
            }
            ManuscriptAction::Delete(id) => {
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    store.delete(id)?;
                    Ok(())
                })();
                self.manuscript_dirty = true;
                match result {
                    Ok(()) => {
                        self.detach_docs_of(id);
                        self.status = format!("已删除稿件 #{id}。");
                    }
                    Err(error) => self.status = format!("删除失败：{error:#}"),
                }
            }
            ManuscriptAction::DeleteSelected(ids) => {
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    store.delete_many(&ids)?;
                    Ok(())
                })();
                self.manuscript_dirty = true;
                match result {
                    Ok(()) => {
                        for id in &ids {
                            self.detach_docs_of(*id);
                            self.manuscript_selected.remove(id);
                        }
                        self.status = format!("已批量删除 {} 篇稿件。", ids.len());
                    }
                    Err(error) => self.status = format!("批量删除失败：{error:#}"),
                }
            }
            ManuscriptAction::DiffVersion {
                manuscript_id,
                version_number,
            } => {
                // 看某一版"改了什么"就是拿它跟上一版比：旧在左、新在右。
                self.version_diff = Some(VersionDiffState {
                    scope: VersionScope::Manuscript(manuscript_id),
                    from: (version_number > 1).then_some(version_number - 1),
                    to: Some(version_number),
                    to_is_current_config: false,
                    pair: crate::version_pair_view::VersionPairViewState::default(),
                });
            }
            ManuscriptAction::OpenVersionDiff { manuscript_id } => {
                let latest = self.manuscript_versions.last().map(|v| v.version_number);
                self.version_diff = Some(VersionDiffState {
                    scope: VersionScope::Manuscript(manuscript_id),
                    from: latest.and_then(|n| (n > 1).then_some(n - 1)),
                    to: latest,
                    to_is_current_config: false,
                    pair: crate::version_pair_view::VersionPairViewState::default(),
                });
            }
            ManuscriptAction::LoadVersion {
                manuscript_id,
                version_number,
            } => self.load_manuscript_version(manuscript_id, version_number),
            ManuscriptAction::RevertPending {
                manuscript_id,
                version_number,
            } => self.revert_confirm = Some((manuscript_id, version_number)),
        }
    }

    pub(crate) fn apply_pdf_action(&mut self, action: PdfAction) {
        match action {
            PdfAction::Open(id) => self.open_pdf_attachment(id),
            PdfAction::SaveAs(id) => self.save_pdf_attachment(id),
            PdfAction::Delete(id) => {
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    store.remove_pdf(id)?;
                    Ok(())
                })();
                self.manuscript_detail_delete_pdf = None;
                self.reload_detail();
                match result {
                    Ok(()) => self.status = "已删除附件。".into(),
                    Err(error) => self.status = format!("删除附件失败：{error:#}"),
                }
            }
        }
    }

    pub(crate) fn transition_status(&mut self, id: i64, target: ManuscriptStatus) {
        let result: anyhow::Result<()> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.set_status(id, target)?;
            Ok(())
        })();
        self.manuscript_dirty = true;
        match result {
            Ok(()) => self.status = format!("稿件已转为{}。", target.label()),
            Err(error) => self.status = format!("状态流转失败：{error:#}"),
        }
    }

    pub(crate) fn refresh_detail(&mut self, id: i64) {
        let result: anyhow::Result<Option<ManuscriptRecord>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.get(id)
        })();
        match result {
            Ok(Some(record)) => {
                self.manuscript_detail = Some(record);
                self.refresh_manuscript_versions(id);
                self.refresh_detail_send_package(id);
            }
            Ok(None) => {
                self.status = "稿件不存在或已被删除。".into();
                self.manuscript_detail = None;
            }
            Err(error) => {
                self.status = format!("读取稿件失败：{error:#}");
                self.manuscript_detail = None;
            }
        }
    }

    /// 载入详情时同步读取该稿件的版本历史列表。
    pub(crate) fn refresh_manuscript_versions(&mut self, id: i64) {
        self.manuscript_versions = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.list_manuscript_versions(id).ok())
            .unwrap_or_default();
    }

    /// 载入详情时同步读取送批材料（呈批件的清单标题）与引用它的呈批件。
    pub(crate) fn refresh_detail_send_package(&mut self, id: i64) {
        self.manuscript_package_titles.clear();
        self.manuscript_referrers.clear();
        let Some(store) = self.manuscript_store.as_ref() else {
            return;
        };
        if let Ok(items) = store.send_package_items(id) {
            self.manuscript_package_titles = items
                .iter()
                .map(|item| {
                    item.manuscript_id
                        .and_then(|item_id| store.manuscript_brief(item_id).ok().flatten())
                        .map(|brief| brief.title)
                        .unwrap_or_else(|| "（本机未找到该稿件）".into())
                })
                .collect();
        }
        self.manuscript_referrers = store.send_package_referrers(id).unwrap_or_default();
    }

    /// 按当前勾选算出批量删除的范围并冻结：归档稿件、被已归档呈批件用作送批材料的
    /// 稿件都保留不动，其余才删。
    pub(crate) fn prepare_batch_delete(&self) -> BatchDeleteConfirm {
        let used_by_archived = |id: i64| {
            self.manuscript_store.as_ref().is_some_and(|store| {
                store.send_package_referrers(id).is_ok_and(|refs| {
                    refs.iter()
                        .any(|owner| owner.status == ManuscriptStatus::Archived)
                })
            })
        };
        let candidates = self
            .manuscript_rows
            .iter()
            .filter(|row| {
                self.manuscript_selected.contains(&row.id)
                    && row.status != ManuscriptStatus::Archived
            })
            .map(|row| row.id)
            .collect::<Vec<_>>();
        let (blocked, deletable): (Vec<i64>, Vec<i64>) =
            candidates.into_iter().partition(|&id| used_by_archived(id));
        BatchDeleteConfirm {
            archived: self
                .manuscript_selected
                .len()
                .saturating_sub(deletable.len() + blocked.len()),
            packaged: blocked.len(),
            deletable,
        }
    }

    /// ZIP 密码框点了提交：校验通过就执行，否则把错误写回框里。
    pub(crate) fn submit_zip_password(&mut self) {
        let Some(dialog) = self.manuscript_zip_password.as_mut() else {
            return;
        };
        let validation = if dialog.purpose.is_import() {
            (!dialog.password.is_empty())
                .then_some(())
                .ok_or_else(|| anyhow::anyhow!("请输入 ZIP 密码"))
        } else if dialog.password != dialog.confirmation {
            Err(anyhow::anyhow!("两次输入的密码不一致"))
        } else {
            manuscript_io::validate_export_password(&dialog.password)
        };
        if let Err(error) = validation {
            dialog.error = Some(format!("{error:#}"));
            return;
        }
        let dialog = self.manuscript_zip_password.take().unwrap();
        self.run_zip_password_action(
            dialog.purpose,
            dialog.password,
            dialog.remember,
            dialog.include_send_package,
        );
    }

    pub(crate) fn reload_detail(&mut self) {
        let Some(id) = self.manuscript_detail.as_ref().map(|d| d.id) else {
            return;
        };
        self.refresh_detail(id);
    }

    /// 打开稿件。已经开着就切到那个标签，否则新开一个。不自动改状态
    /// （打开查看不翻状态）。
    pub(crate) fn open_in_editor(&mut self, id: i64) {
        if self.focus_manuscript(id) {
            let title = self.doc().title();
            self.status = format!("已切换到已打开的《{title}》。");
            return;
        }
        let result: anyhow::Result<Option<ManuscriptRecord>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.get(id)
        })();
        match result {
            Ok(Some(record)) => {
                let title = record.title.clone();
                let record_status = record.status;
                let mut session = DraftSession::from_parts(
                    0,
                    Some(record.id),
                    record.snapshot,
                    record.content_markdown,
                );
                session.record_status = record.status;
                session.mark_saved();
                self.open_doc(session);
                self.refresh_committed_baseline(self.active_doc);
                self.status = match record_status {
                    ManuscriptStatus::Published => {
                        format!("已打开已发布稿件《{title}》，当前为只读；退回草稿后可编辑。")
                    }
                    ManuscriptStatus::Archived => {
                        format!("已打开归档稿件《{title}》，当前为只读。")
                    }
                    _ => format!("已打开稿件《{title}》，可继续编辑并保存。"),
                };
            }
            Ok(None) => self.status = "稿件不存在或已被删除。".into(),
            Err(error) => self.status = format!("载入稿件失败：{error:#}"),
        }
    }

    /// 复制现有稿件为一份尚未入库的新稿，开在新标签里。只继承可编辑内容，
    /// 不继承原记录身份、生命周期状态、版本历史和 PDF 附件；首次保存会走
    /// 新建分支，绝不覆盖来源稿。
    pub(crate) fn create_from_existing(&mut self, id: i64) {
        let result: anyhow::Result<Option<ManuscriptRecord>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.get(id)
        })();
        match result {
            Ok(Some(record)) => {
                let title = record.title.clone();
                self.open_doc(DraftSession::from_parts(
                    0,
                    None,
                    record.snapshot,
                    record.content_markdown,
                ));
                self.status = format!(
                    "已基于《{title}》新建公文。修改后点“保存到稿件库”将新增一条草稿，不会覆盖原稿。"
                );
            }
            Ok(None) => self.status = "来源稿件不存在或已被删除。".into(),
            Err(error) => self.status = format!("基于现有公文新建失败：{error:#}"),
        }
    }

    /// 起草页“保存到稿件库”：新建记录或更新当前打开的记录。
    pub(crate) fn save_to_manuscript_library(&mut self) {
        let _ = self.try_save_to_manuscript_library();
    }

    /// 备份前也复用普通保存路径，但必须把失败交给调用方，防止导出旧内容。
    pub(crate) fn try_save_to_manuscript_library(&mut self) -> anyhow::Result<()> {
        let normalized = export::normalize_ordered_list_punctuation(&self.doc().generated_markdown);
        self.doc_mut().generated_markdown = normalized;
        // 表单刚改过而正文未动时也要在保存点重跑函号等元数据规则。
        self.draft_page().revalidate();
        let snapshot = self.doc().draft.clone();
        let content = self.doc().generated_markdown.clone();
        let title = export::document_title(&snapshot, &content);
        let current_id = self.doc().manuscript_id;
        let result: anyhow::Result<String> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用，无法保存"))?;
            if let Some(id) = current_id {
                let record = store
                    .get(id)?
                    .ok_or_else(|| anyhow::anyhow!("稿件不存在或已被删除"))?;
                if record.status == ManuscriptStatus::Archived {
                    anyhow::bail!("归档稿件不可修改，请在稿件管理页查看。");
                }
                if record.status == ManuscriptStatus::Published {
                    anyhow::bail!("该稿件已发布，请先在稿件管理页“退回草稿”再修改。");
                }
                store.update(
                    id,
                    &ManuscriptUpdate {
                        snapshot,
                        content_markdown: content,
                        notes: record.notes,
                    },
                )?;
                Ok(format!("已更新稿件《{title}》。"))
            } else {
                let new_id = store.create(
                    &NewManuscript {
                        snapshot,
                        content_markdown: content,
                        notes: String::new(),
                        status: ManuscriptStatus::Draft,
                        ..Default::default()
                    },
                    None,
                )?;
                self.doc_mut().manuscript_id = Some(new_id);
                Ok(format!(
                    "已保存为草稿《{title}》，后续保存会更新同一条记录。"
                ))
            }
        })();
        self.manuscript_dirty = true;
        match result {
            Ok(message) => {
                self.doc_mut().mark_saved();
                self.refresh_committed_baseline(self.active_doc);
                self.status = message;
                Ok(())
            }
            Err(error) => {
                self.status = format!("保存失败：{error:#}");
                Err(error)
            }
        }
    }

    /// 新开一篇空白稿件的标签。
    pub(crate) fn new_blank_manuscript(&mut self) {
        let session = DraftSession::blank(0, &self.config);
        self.open_doc(session);
        self.status = "已新建空白稿件：点“保存到稿件库”将新增一条草稿记录。".into();
    }

    /// 选一个现成文档，把它转成 markdown，作为一篇新稿件打开。
    /// 只带正文——抬头、文号这些要素还得在左侧要素区自己填。
    pub(crate) fn new_manuscript_from_document(&mut self) {
        let Some(path) = doc_import::pick_file() else {
            return;
        };
        match doc_import::to_draft_markdown(&path, true) {
            Ok(markdown) => {
                let chars = markdown.chars().count();
                let session = DraftSession::with_markdown(0, &self.config, markdown);
                self.open_doc(session);
                self.status = format!(
                    "已从 {} 新建稿件（{chars} 字）：请核对正文结构并补齐左侧文档要素。",
                    doc_import::file_label(&path)
                );
                self.status.push_str(doc_import::import_notice(&path));
            }
            Err(error) => self.status = format!("导入失败：{error:#}"),
        }
    }

    /// 按文件名顺序合并目录第一层的 Markdown，新建一篇研究报告。
    pub(crate) fn new_research_manuscript_from_folder(&mut self) {
        let Some(folder) = doc_import::pick_folder() else {
            return;
        };
        match doc_import::import_research_folder(&folder) {
            Ok(imported) => {
                let chars = imported.markdown.chars().count();
                let count = imported.file_names.len();
                let names = imported.file_names.join("、");
                let session = DraftSession::with_research_markdown(
                    0,
                    imported.markdown,
                    imported.title,
                    imported.metadata,
                );
                let skipped = imported.skipped_images.clone();
                self.open_doc(session);
                self.status =
                    format!("已按文件名顺序合并 {count} 个 Markdown（{chars} 字）：{names}");
                // 缺图不阻断导入，但必须说出来：正文里这些引用还指着原文件夹，
                // 不补齐的话编译 PDF 时才会失败。
                if !skipped.is_empty() {
                    self.status.push_str(&format!(
                        "；{} 张图片未能入库，请补齐后重新插入：{}",
                        skipped.len(),
                        skipped.join("、")
                    ));
                }
            }
            Err(error) => self.status = format!("研究报告文件夹导入失败：{error:#}"),
        }
    }
}
