//! 稿件库页：列表/详情、导入导出、PDF 附件与状态流转。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::manuscript_ui`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{
    FORM_CONTROL_HEIGHT, GongwenApp, VersionDiffState, VersionScope, WorkerResult, accent,
    joined_metadata, metadata_grid_row, present_or_dash, security_level_color,
    security_level_list_label, short_date, status_color, summarize, truncate, warn,
};
use crate::diff_view::DiffViewState;
use crate::doc_import;
use crate::draft_page::DraftSession;
use crate::export;
use crate::manuscript::{
    ManuscriptFilter, ManuscriptRecord, ManuscriptStore, ManuscriptUpdate, NewManuscript,
};
use crate::manuscript_io;
use crate::models::{ManuscriptStatus, SecurityLevel, TemplateKind, VocabularyCategory};
use crate::storage;
use crate::theme;
use crate::units;
use crate::vocabulary_xlsx;
use anyhow::Context;
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::thread;

/// 稿件列表上的操作在遍历表格时不能直接改 `self`，先记下来循环结束后再执行。
pub(crate) enum ManuscriptAction {
    /// 打开只读详情（归档行或“查看”）。
    Detail(i64),
    /// 载入起草页继续编辑。
    Edit(i64),
    /// 复制该稿件的行文要素和正文，作为一篇尚未入库的新稿载入起草页。
    CreateFromExisting(i64),
    Publish(i64),
    RevertToDraft(i64),
    /// 归档。PDF 附件来自 `manuscript_archive_pending` 中已选的文件。
    Archive(i64),
    Delete(i64),
    DeleteSelected(Vec<i64>),
    /// 进入删除二次确认。
    DeletePending(i64),
    /// 进入归档确认（先选扫描盖章 PDF）。
    ArchivePending(i64),
    /// 打开版本对照窗，对照该版本与其上一版（旧在左、新在右）。
    DiffVersion {
        manuscript_id: i64,
        version_number: i64,
    },
    /// 打开版本对照窗，默认对照最新版与上一版。
    OpenVersionDiff {
        manuscript_id: i64,
    },
    /// 把某版本载入起草页继续编辑（不写活稿行）。
    LoadVersion {
        manuscript_id: i64,
        version_number: i64,
    },
    /// 进入"回退到该版本"的二次确认。
    RevertPending {
        manuscript_id: i64,
        version_number: i64,
    },
}

/// 详情窗内 PDF 附件的操作，同样延迟到帧末执行。
pub(crate) enum PdfAction {
    Open(i64),
    SaveAs(i64),
    Delete(i64),
}

/// 等待二次确认的归档操作：记录要归档的稿件与已选择的扫描盖章 PDF。
pub(crate) struct ArchivePending {
    manuscript_id: i64,
    pdf_paths: Vec<PathBuf>,
}

/// 「导出 PDF」选项弹窗的勾选状态：盖章件取附件、非盖章件编译生成。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PdfExportDialog {
    stamped: bool,
    compiled: bool,
}

#[derive(Debug, Clone)]
enum ZipPasswordPurpose {
    FilteredExport,
    SelectedExport,
    PdfExport(manuscript_io::PdfExportOptions),
    Import(PathBuf),
}

impl ZipPasswordPurpose {
    fn is_import(&self) -> bool {
        matches!(self, Self::Import(_))
    }
}

/// 每次 ZIP 导入/导出都显示的密码弹窗。导出要二次确认并检查强度；导入只验证非空，
/// 以便兼容由其他工具生成的历史压缩包。
pub(crate) struct ZipPasswordDialog {
    purpose: ZipPasswordPurpose,
    password: String,
    confirmation: String,
    remember: bool,
    show_password: bool,
    error: Option<String>,
}

impl ZipPasswordDialog {
    fn new(purpose: ZipPasswordPurpose, remembered: Option<&str>) -> Self {
        let password = remembered.unwrap_or_default().to_string();
        Self {
            purpose,
            confirmation: password.clone(),
            password,
            remember: remembered.is_some(),
            show_password: false,
            error: None,
        }
    }
}

/// 导入 ZIP 的预览状态：清单、勾选、关键词过滤与是否跳过同源记录。
pub(crate) struct ImportPreview {
    manifest: manuscript_io::Manifest,
    zip_path: PathBuf,
    selected: Vec<bool>,
    keyword: String,
    skip_existing: bool,
    /// 包内随附的标准词库；旧包无此条目时为 None。
    vocabulary: Option<manuscript_io::VocabularyFile>,
    /// 是否把包内词库增量合并到本机词库，默认勾选。
    merge_vocabulary: bool,
    /// 仅在这次预览/确认导入期间驻留内存，不写入稿件或应用配置。
    password: String,
}

impl GongwenApp {
    pub(crate) fn manuscript_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.manuscript_error.clone() {
            ui.colored_label(warn(), error);
            return;
        }
        if self.manuscript_store.is_none() {
            ui.colored_label(warn(), "稿件库不可用。");
            return;
        }

        let mut action: Option<ManuscriptAction> = None;
        const SINGLE_PAGE_DETAIL_MAX_WIDTH: f32 = 720.0;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("稿件管理");
                let total: i64 = self.manuscript_count.iter().sum();
                ui.weak(format!(
                    "共 {total} 篇 · 草稿 {} · 发布 {} · 归档 {} · 仅保存在本机",
                    self.manuscript_count[1], self.manuscript_count[2], self.manuscript_count[3],
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(theme::Icon::FilePlus, "新建稿件"))
                    .on_hover_text("清空起草页，开始一份全新的稿件")
                    .clicked()
                {
                    self.new_blank_manuscript();
                }
            });
        });
        ui.add_space(8.0);

        let single_page_detail = self.manuscript_detail.is_some()
            && ui.available_width() <= SINGLE_PAGE_DETAIL_MAX_WIDTH;
        if !single_page_detail {
            self.manuscript_filter_bar(ui);
            self.refresh_manuscript_rows();
            self.manuscript_confirm_groups(ui, &mut action);
        }

        // 详情优先取得稳定宽度，列表使用剩余区域。双栏不再依赖 Panel 的隐式
        // 分配顺序，而是显式切成两个带独立 clip_rect 的子 Ui：表格先画、详情
        // 后画。即使表格内部列宽超出，也只能在左侧横向滚动，绝不会盖住详情。
        // 窄屏仍把详情作为独立二级页面，避免两块内容都窄到不可用。
        const WIDE_DETAIL_MIN_WIDTH: f32 = 1180.0;
        let body_rect = ui.available_rect_before_wrap();
        let body_width = body_rect.width();
        let detail_open = self.manuscript_detail.is_some();
        let mut pdf_action = None;

        if !detail_open {
            let compact = body_width <= SINGLE_PAGE_DETAIL_MAX_WIDTH;
            let horizontal_scroll = !compact && body_width < WIDE_DETAIL_MIN_WIDTH;
            self.manuscript_list_ui(ui, &mut action, compact, horizontal_scroll);
        } else if body_width > SINGLE_PAGE_DETAIL_MAX_WIDTH {
            const COLUMN_GAP: f32 = 8.0;
            let detail_width = (body_width * 0.30).clamp(320.0, 400.0);
            let list_right = body_rect.right() - detail_width - COLUMN_GAP;
            let list_rect =
                egui::Rect::from_min_max(body_rect.min, egui::pos2(list_right, body_rect.bottom()));
            let detail_rect = egui::Rect::from_min_max(
                egui::pos2(list_right + COLUMN_GAP, body_rect.top()),
                body_rect.max,
            );

            let mut list_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(list_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            list_ui.set_clip_rect(list_rect);
            self.manuscript_list_ui(&mut list_ui, &mut action, false, true);

            // 详情在同层最后绘制，并先铺满不透明底色；clip_rect 同时限制绘制
            // 与命中区域，形成真正互不越界的宽屏双栏。
            let mut detail_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(detail_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            detail_ui.set_clip_rect(detail_rect);
            detail_ui
                .painter()
                .rect_filled(detail_rect, 0.0, theme::surface());
            detail_ui.painter().line_segment(
                [detail_rect.left_top(), detail_rect.left_bottom()],
                egui::Stroke::new(1.0, theme::border()),
            );
            pdf_action = theme::panel(theme::surface(), 10)
                .show(&mut detail_ui, |ui| {
                    self.manuscript_detail_ui(ui, &mut action, false)
                })
                .inner;
            ui.advance_cursor_after_rect(body_rect);
        } else {
            pdf_action = self.manuscript_detail_ui(ui, &mut action, true);
        }

        if let Some(act) = action {
            self.apply_manuscript_action(act);
        }
        if let Some(act) = pdf_action {
            self.apply_pdf_action(act);
        }
    }

    pub(crate) fn manuscript_filter_bar(&mut self, ui: &mut egui::Ui) {
        // 文本框、下拉框和按钮的默认内容边距不同。统一交互高度后，各筛选组的
        // 外框高度一致，组内标签也会落在同一条垂直中线上。
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = FORM_CONTROL_HEIGHT;
            ui.horizontal_wrapped(|ui| {
                ui.horizontal(|ui| {
                    ui.label("搜索");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.manuscript_filter.keyword)
                                .hint_text("标题、文号或备注")
                                .desired_width(220.0),
                        )
                        .changed()
                    {
                        self.manuscript_dirty = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("状态");
                    egui::ComboBox::from_id_salt("manuscript_status")
                        .selected_text(
                            self.manuscript_filter
                                .status
                                .map(|s| s.label())
                                .unwrap_or("全部状态"),
                        )
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut self.manuscript_filter.status,
                                    None,
                                    "全部状态",
                                )
                                .changed()
                            {
                                self.manuscript_dirty = true;
                            }
                            for status in ManuscriptStatus::ALL {
                                if status == ManuscriptStatus::New {
                                    continue; // “新建”已取消，历史记录创建时统一升为草稿。
                                }
                                if ui
                                    .selectable_value(
                                        &mut self.manuscript_filter.status,
                                        Some(status),
                                        status.label(),
                                    )
                                    .changed()
                                {
                                    self.manuscript_dirty = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("文种");
                    egui::ComboBox::from_id_salt("manuscript_kind")
                        .selected_text(
                            self.manuscript_filter
                                .kind
                                .map(|k| k.label())
                                .unwrap_or("全部文种"),
                        )
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut self.manuscript_filter.kind,
                                    None,
                                    "全部文种",
                                )
                                .changed()
                            {
                                self.manuscript_dirty = true;
                            }
                            for kind in TemplateKind::ALL {
                                if ui
                                    .selectable_value(
                                        &mut self.manuscript_filter.kind,
                                        Some(kind),
                                        kind.label(),
                                    )
                                    .changed()
                                {
                                    self.manuscript_dirty = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("成文日期");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.manuscript_filter.date_from)
                                .hint_text("起 YYYY-MM-DD")
                                .desired_width(110.0),
                        )
                        .changed()
                    {
                        self.manuscript_dirty = true;
                    }
                    ui.label("至");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.manuscript_filter.date_to)
                                .hint_text("止 YYYY-MM-DD")
                                .desired_width(110.0),
                        )
                        .changed()
                    {
                        self.manuscript_dirty = true;
                    }
                });
                let filter_active = !self.manuscript_filter.keyword.trim().is_empty()
                    || self.manuscript_filter.status.is_some()
                    || self.manuscript_filter.kind.is_some()
                    || !self.manuscript_filter.date_from.trim().is_empty()
                    || !self.manuscript_filter.date_to.trim().is_empty();
                if filter_active
                    && theme::icon_button(ui, theme::Icon::SearchClear, "清除筛选").clicked()
                {
                    self.manuscript_filter = ManuscriptFilter::default();
                    self.manuscript_dirty = true;
                }
                ui.separator();
                let selected = self.manuscript_selected.len();
                if selected > 0 {
                    ui.label(format!("已选 {selected} 篇"));
                    if ui
                        .add(theme::icon_text_button(theme::Icon::Package, "导出所选"))
                        .on_hover_text("仅导出列表中已勾选的稿件及其 PDF 附件")
                        .clicked()
                    {
                        self.export_selected_manuscripts_zip();
                    }
                    if ui
                        .add_enabled(
                            !self.manuscript_pdf_export_busy,
                            theme::icon_text_button(theme::Icon::FileDown, "导出 PDF"),
                        )
                        .on_hover_text(
                            "把勾选的稿件导出为 PDF 并打包 zip：盖章件直接取附件、非盖章件编译生成",
                        )
                        .clicked()
                    {
                        self.manuscript_pdf_export = Some(PdfExportDialog {
                            stamped: true,
                            compiled: true,
                        });
                    }
                    if ui
                        .add(theme::icon_text_button(
                            theme::Icon::PackageOpen,
                            "导入到知识库",
                        ))
                        .on_hover_text("把勾选的稿件切块、向量化后加入知识库，供起草时检索参考")
                        .clicked()
                    {
                        self.knowledge_import_selected_manuscripts();
                    }
                    let deletable = self.manuscript_rows.iter().any(|row| {
                        self.manuscript_selected.contains(&row.id)
                            && row.status != ManuscriptStatus::Archived
                    });
                    if ui
                        .add_enabled(
                            deletable,
                            theme::warning_icon_button(theme::Icon::Trash, "批量删除"),
                        )
                        .on_hover_text("归档稿件不会被删除")
                        .clicked()
                    {
                        self.manuscript_batch_delete_confirm = true;
                    }
                    if theme::icon_button(ui, theme::Icon::X, "清空选择").clicked() {
                        self.manuscript_selected.clear();
                    }
                    ui.separator();
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::Package, "按筛选导出"))
                    .on_hover_text("按当前过滤条件导出稿件（含 PDF 附件）")
                    .clicked()
                {
                    self.export_manuscripts_zip();
                }
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::PackageOpen,
                        "导入 ZIP",
                    ))
                    .on_hover_text("从 ZIP 稿件包导入，先预览后确认")
                    .clicked()
                {
                    self.start_import_manuscript();
                }
            });
        });
        ui.add_space(6.0);
    }

    pub(crate) fn refresh_manuscript_rows(&mut self) {
        // 「已入库」标记要在稿件管理页上也是准的，哪怕用户从没打开过知识库页。
        if self.knowledge_dirty
            && let Some(knowledge) = self.knowledge_store.as_mut()
        {
            self.knowledge_indexed_manuscripts =
                knowledge.indexed_manuscript_ids().unwrap_or_default();
        }
        let Some(store) = self.manuscript_store.as_mut() else {
            return;
        };
        if self.manuscript_applied != Some(self.manuscript_filter.clone()) || self.manuscript_dirty
        {
            match store.list(&self.manuscript_filter) {
                Ok(rows) => {
                    self.manuscript_rows = rows;
                    let visible = self
                        .manuscript_rows
                        .iter()
                        .map(|row| row.id)
                        .collect::<BTreeSet<_>>();
                    self.manuscript_selected.retain(|id| visible.contains(id));
                    if self
                        .manuscript_detail
                        .as_ref()
                        .is_some_and(|detail| !visible.contains(&detail.id))
                    {
                        self.manuscript_detail = None;
                        self.manuscript_detail_delete_pdf = None;
                        self.manuscript_versions.clear();
                    } else if let Some(detail_id) =
                        self.manuscript_detail.as_ref().map(|detail| detail.id)
                        && let Ok(Some(record)) = store.get(detail_id)
                    {
                        // 发布、退回草稿或归档后，右侧状态和时间立即跟着列表更新。
                        self.manuscript_detail = Some(record);
                    }
                    self.manuscript_count = store.count_by_status().unwrap_or([0; 4]);
                    self.manuscript_dirty = false;
                    self.manuscript_applied = Some(self.manuscript_filter.clone());
                }
                Err(error) => self.status = format!("查询稿件失败：{error:#}"),
            }
        }
    }

    /// 删除 / 归档 / 导入预览三组确认区。可能写入 `action`，在帧末执行。
    pub(crate) fn manuscript_confirm_groups(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<ManuscriptAction>,
    ) {
        if let Some(id) = self.manuscript_delete_confirm {
            let mut do_delete = false;
            let mut do_cancel = false;
            ui.group(|ui| {
                ui.colored_label(warn(), "删除后不可恢复，确认删除这篇稿件吗？");
                ui.horizontal(|ui| {
                    if ui.button("确认删除").clicked() {
                        do_delete = true;
                    }
                    if ui.button("取消").clicked() {
                        do_cancel = true;
                    }
                });
            });
            if do_cancel {
                self.manuscript_delete_confirm = None;
            } else if do_delete {
                self.manuscript_delete_confirm = None;
                *action = Some(ManuscriptAction::Delete(id));
            }
            ui.add_space(6.0);
        }

        if self.manuscript_batch_delete_confirm {
            let deletable = self
                .manuscript_rows
                .iter()
                .filter(|row| {
                    self.manuscript_selected.contains(&row.id)
                        && row.status != ManuscriptStatus::Archived
                })
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let archived = self
                .manuscript_selected
                .len()
                .saturating_sub(deletable.len());
            let mut confirm = false;
            let mut cancel = false;
            ui.group(|ui| {
                ui.colored_label(
                    warn(),
                    format!(
                        "确认删除所选的 {} 篇可删除稿件吗？此操作不可恢复。",
                        deletable.len()
                    ),
                );
                if archived > 0 {
                    ui.weak(format!("另有 {archived} 篇归档稿件受保护，将保留不动。"));
                }
                ui.horizontal(|ui| {
                    if ui.button("确认批量删除").clicked() {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
            if cancel {
                self.manuscript_batch_delete_confirm = false;
            } else if confirm {
                self.manuscript_batch_delete_confirm = false;
                *action = Some(ManuscriptAction::DeleteSelected(deletable));
            }
            ui.add_space(6.0);
        }

        if let Some(mut dialog) = self.manuscript_pdf_export {
            let mut confirm = false;
            let mut cancel = false;
            ui.group(|ui| {
                ui.label(format!(
                    "已选 {} 篇稿件，导出为 PDF 压缩包：",
                    self.manuscript_selected.len()
                ));
                ui.checkbox(&mut dialog.stamped, "导出盖章件（直接取附件 PDF）");
                ui.checkbox(&mut dialog.compiled, "导出非盖章件（编译生成 PDF）");
                ui.weak("文件名按稿件导出命名（不含时间戳）；盖章件名称追加（盖章）。");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            dialog.stamped || dialog.compiled,
                            theme::icon_text_button(theme::Icon::FileDown, "导出"),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
            if cancel {
                self.manuscript_pdf_export = None;
            } else if confirm {
                let options = manuscript_io::PdfExportOptions {
                    stamped: dialog.stamped,
                    compiled: dialog.compiled,
                };
                self.manuscript_pdf_export = None;
                self.open_zip_password_dialog(ZipPasswordPurpose::PdfExport(options));
            }
            ui.add_space(6.0);
        }

        if self.manuscript_zip_password.is_some() {
            let mut submit = false;
            let mut cancel = false;
            {
                let dialog = self.manuscript_zip_password.as_mut().unwrap();
                ui.group(|ui| {
                    let importing = dialog.purpose.is_import();
                    ui.strong(if importing {
                        "输入 ZIP 密码"
                    } else {
                        "设置 ZIP 加密密码"
                    });
                    ui.label(if importing {
                        "必须输入正确密码后才能读取和导入稿件包。"
                    } else {
                        "本次导出的 ZIP 将使用 AES-256 加密，所有文件均受密码保护。"
                    });
                    ui.horizontal(|ui| {
                        ui.label("密码");
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.password)
                                .password(!dialog.show_password)
                                .desired_width(260.0),
                        );
                    });
                    if !importing {
                        ui.horizontal(|ui| {
                            ui.label("确认");
                            ui.add(
                                egui::TextEdit::singleline(&mut dialog.confirmation)
                                    .password(!dialog.show_password)
                                    .desired_width(260.0),
                            );
                        });
                        ui.weak(
                            "至少 10 位，并包含大写字母、小写字母、数字、符号或中文中的至少三类；不能使用常见口令、连续或重复字符。",
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut dialog.remember, "记住密码")
                            .on_hover_text("保存在本机受限权限文件中，下次仍会显示确认窗口");
                        ui.checkbox(&mut dialog.show_password, "显示密码");
                    });
                    if let Some(error) = &dialog.error {
                        ui.colored_label(warn(), error);
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !dialog.password.is_empty(),
                                theme::icon_text_button(
                                    if importing {
                                        theme::Icon::PackageOpen
                                    } else {
                                        theme::Icon::Package
                                    },
                                    if importing { "解密并预览" } else { "继续导出" },
                                ),
                            )
                            .clicked()
                        {
                            submit = true;
                        }
                        if ui.button("取消").clicked() {
                            cancel = true;
                        }
                    });
                });
            }
            if cancel {
                self.manuscript_zip_password = None;
            } else if submit {
                let validation = {
                    let dialog = self.manuscript_zip_password.as_ref().unwrap();
                    if dialog.purpose.is_import() {
                        (!dialog.password.is_empty())
                            .then_some(())
                            .ok_or_else(|| anyhow::anyhow!("请输入 ZIP 密码"))
                    } else if dialog.password != dialog.confirmation {
                        Err(anyhow::anyhow!("两次输入的密码不一致"))
                    } else {
                        manuscript_io::validate_export_password(&dialog.password)
                    }
                };
                match validation {
                    Ok(()) => {
                        let dialog = self.manuscript_zip_password.take().unwrap();
                        self.run_zip_password_action(
                            dialog.purpose,
                            dialog.password,
                            dialog.remember,
                        );
                    }
                    Err(error) => {
                        self.manuscript_zip_password.as_mut().unwrap().error =
                            Some(format!("{error:#}"));
                    }
                }
            }
            ui.add_space(6.0);
        }

        let mut archive_to_confirm: Option<i64> = None;
        if self.manuscript_archive_pending.is_some() {
            let (manuscript_id, do_archive, do_cancel) = {
                let pending = self.manuscript_archive_pending.as_mut().unwrap();
                let manuscript_id = pending.manuscript_id;
                let mut do_archive = false;
                let mut do_cancel = false;
                ui.group(|ui| {
                    ui.colored_label(
                        warn(),
                        "归档将冻结该稿件：标题、正文、时间等关键信息此后均不可修改。",
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add(theme::icon_text_button(
                                theme::Icon::Paperclip,
                                "选择扫描盖章 PDF…",
                            ))
                            .on_hover_text("可多选；归档后仍可在详情页继续添加附件")
                            .clicked()
                            && let Some(paths) = rfd::FileDialog::new()
                                .add_filter("扫描盖章 PDF", &["pdf"])
                                .pick_files()
                        {
                            pending.pdf_paths.extend(paths);
                        }
                        if !pending.pdf_paths.is_empty() {
                            ui.label(format!("已选 {} 个 PDF：", pending.pdf_paths.len()));
                            for path in pending.pdf_paths.iter() {
                                let name = path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("PDF")
                                    .to_string();
                                ui.label(format!("  {name}"));
                            }
                        }
                        ui.separator();
                        if ui.button("确认归档").clicked() {
                            do_archive = true;
                        }
                        if ui.button("取消").clicked() {
                            do_cancel = true;
                        }
                    });
                });
                (manuscript_id, do_archive, do_cancel)
            };
            if do_cancel {
                self.manuscript_archive_pending = None;
            } else if do_archive {
                // 保留 pending（含已选 PDF），由 Archive action 读取并执行归档。
                archive_to_confirm = Some(manuscript_id);
            }
            ui.add_space(6.0);
        }
        if let Some(id) = archive_to_confirm {
            *action = Some(ManuscriptAction::Archive(id));
        }

        if self.manuscript_import_preview.is_some() {
            let (confirm, cancel) = {
                let preview = self.manuscript_import_preview.as_mut().unwrap();
                let mut confirm = false;
                let mut cancel = false;
                ui.group(|ui| {
                    let total = preview.manifest.records.len();
                    let selected = preview.selected.iter().filter(|b| **b).count();
                    let archived = preview
                        .manifest
                        .records
                        .iter()
                        .filter(|r| r.status == ManuscriptStatus::Archived)
                        .count();
                    ui.strong("导入预览");
                    ui.label(format!(
                        "共 {total} 篇（归档 {archived} 篇），已勾选 {selected} 篇。"
                    ));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut preview.keyword)
                                .hint_text("按标题/文号过滤")
                                .desired_width(220.0),
                        );
                        if ui
                            .add(theme::icon_text_button(theme::Icon::SquareCheck, "全选"))
                            .clicked()
                        {
                            for b in preview.selected.iter_mut() {
                                *b = true;
                            }
                        }
                        if ui
                            .add(theme::icon_text_button(theme::Icon::Square, "全不选"))
                            .clicked()
                        {
                            for b in preview.selected.iter_mut() {
                                *b = false;
                            }
                        }
                    });
                    ui.checkbox(&mut preview.skip_existing, "跳过与本地同源的已有记录")
                        .on_hover_text("按清单里的源 id 去重，重复导入同一份文件不会产生副本");
                    if let Some(vocabulary) = &preview.vocabulary {
                        let units = vocabulary
                            .entries
                            .iter()
                            .filter(|entry| entry.category == VocabularyCategory::Unit)
                            .count();
                        let people = vocabulary.entries.len() - units;
                        ui.checkbox(
                            &mut preview.merge_vocabulary,
                            format!(
                                "合并包内标准词库到本机（{} 个单位、{} 名人员）",
                                units, people
                            ),
                        )
                        .on_hover_text(
                            "增量合并：补全本机缺失的词条并更新已匹配词条的补充字段，不删除本机已有内容；取消勾选则不导入词库",
                        );
                    }
                    let keyword = preview.keyword.trim().to_lowercase();
                    egui::ScrollArea::vertical()
                        .id_salt("import_preview_list")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for (index, record) in preview.manifest.records.iter().enumerate() {
                                if !keyword.is_empty()
                                    && !record.title.to_lowercase().contains(&keyword)
                                    && !record.doc_number.to_lowercase().contains(&keyword)
                                {
                                    continue;
                                }
                                ui.checkbox(
                                    &mut preview.selected[index],
                                    format!(
                                        "{} · {} · {}（{}）",
                                        record.doc_date,
                                        record.kind.label(),
                                        truncate(&record.title, 40),
                                        record.status.label(),
                                    ),
                                );
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.button("确认导入").clicked() {
                            confirm = true;
                        }
                        if ui.button("取消").clicked() {
                            cancel = true;
                        }
                    });
                });
                (confirm, cancel)
            };
            if cancel {
                self.manuscript_import_preview = None;
            } else if confirm {
                self.confirm_import();
            }
            ui.add_space(6.0);
        }
    }

    pub(crate) fn manuscript_list_ui(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<ManuscriptAction>,
        compact: bool,
        horizontal_scroll: bool,
    ) {
        if self.manuscript_rows.is_empty() {
            ui.add_space(12.0);
            ui.weak("没有符合条件的稿件。在起草页点“保存到稿件库”，或调整过滤条件。");
            return;
        }
        if horizontal_scroll {
            egui::ScrollArea::horizontal()
                .id_salt("manuscript_table_horizontal")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // 完整列的舒适宽度。外层区域不足时只滚动列表，右侧详情面板
                    // 已提前从父 Ui 划走宽度，因此两者永远不会互相覆盖。
                    ui.set_min_width(1080.0);
                    self.manuscript_table_ui(ui, action, false);
                });
            return;
        }
        self.manuscript_table_ui(ui, action, compact);
    }

    pub(crate) fn manuscript_table_ui(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<ManuscriptAction>,
        compact: bool,
    ) {
        // 表格列宽支持拖拽调整（resizable）：勾选列按内容自适应，操作列吃剩余宽度
        // 且不可拖拽（避免把操作按钮挤出可视区），其余列可拖拽并设最小宽度。
        const ROW_HEIGHT: f32 = 26.0;
        let ctx = ui.ctx().clone();
        let mut table = TableBuilder::new(ui)
            .id_salt(("manuscript_table", compact))
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        table = if compact {
            table
                .column(Column::auto().at_least(28.0)) // 勾选
                .column(Column::initial(52.0).at_least(44.0)) // 状态
                .column(Column::initial(280.0).at_least(160.0)) // 标题
                .column(Column::initial(150.0).at_least(100.0)) // 文号
                .column(Column::initial(76.0).at_least(56.0)) // 成文日期
                .column(Column::remainder().at_least(44.0).resizable(false)) // 更多操作
        } else {
            table
                .column(Column::auto().at_least(28.0)) // 勾选
                .column(Column::initial(52.0).at_least(44.0)) // 状态
                .column(Column::initial(60.0).at_least(44.0)) // 文种
                .column(Column::initial(48.0).at_least(40.0)) // 密级
                .column(Column::initial(240.0).at_least(120.0)) // 标题
                .column(Column::initial(160.0).at_least(100.0)) // 文号
                .column(Column::initial(76.0).at_least(56.0)) // 成文日期
                .column(Column::initial(76.0).at_least(56.0)) // 更新
                .column(Column::initial(76.0).at_least(56.0)) // 归档
                .column(Column::initial(56.0).at_least(44.0)) // 知识库
                .column(Column::remainder().at_least(120.0).resizable(false)) // 操作
        };
        table
            .header(ROW_HEIGHT, |mut header| {
                let visible_ids = self
                    .manuscript_rows
                    .iter()
                    .map(|row| row.id)
                    .collect::<Vec<_>>();
                let mut all_selected = !visible_ids.is_empty()
                    && visible_ids
                        .iter()
                        .all(|id| self.manuscript_selected.contains(id));
                header.col(|ui| {
                    if ui
                        .checkbox(&mut all_selected, "")
                        .on_hover_text(if all_selected {
                            "取消选择当前列表全部稿件"
                        } else {
                            "选择当前列表全部稿件"
                        })
                        .changed()
                    {
                        if all_selected {
                            self.manuscript_selected.extend(visible_ids);
                        } else {
                            for id in visible_ids {
                                self.manuscript_selected.remove(&id);
                            }
                        }
                    }
                });
                header.col(|ui| {
                    ui.strong("状态");
                });
                if !compact {
                    header.col(|ui| {
                        ui.strong("文种");
                    });
                    header.col(|ui| {
                        ui.strong("密级");
                    });
                }
                header.col(|ui| {
                    ui.strong("标题");
                });
                header.col(|ui| {
                    ui.strong("文号");
                });
                header.col(|ui| {
                    ui.strong("成文日期");
                });
                if !compact {
                    header.col(|ui| {
                        ui.strong("更新");
                    });
                    header.col(|ui| {
                        ui.strong("归档");
                    });
                    header.col(|ui| {
                        ui.strong("知识库");
                    });
                }
                header.col(|ui| {
                    ui.strong(if compact { "更多" } else { "操作" });
                });
                // 表头悬停提示：列宽其实可拖拽调整，但界面上没有任何暗示，
                // 用户会以为列宽是写死的。response() 是所有表头单元格的并集。
                header
                    .response()
                    .on_hover_text("拖动列头可调整列宽（操作列除外）");
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.manuscript_rows.len(), |mut row| {
                    let index = row.index();
                    let data = &self.manuscript_rows[index];
                    // 新行淡入：新增行首次出现时从不透明 0 平滑升到 1。
                    let seen_t = ctx.animate_bool_with_time(
                        egui::Id::new(("manuscript_row_seen", data.id)),
                        true,
                        theme::anim::SLOW,
                    );
                    let mut batch_selected = self.manuscript_selected.contains(&data.id);
                    let row_selected = self
                        .manuscript_detail
                        .as_ref()
                        .is_some_and(|detail| detail.id == data.id);
                    // 行点击/双击跨多个单元格收集，用 Cell 避免多个 col 闭包争夺可变借用。
                    let row_clicked = std::cell::Cell::new(false);
                    let row_double_clicked = std::cell::Cell::new(false);
                    // 整行选中高亮：set_selected 让 StripLayout 给整行（含单元格
                    // 间隙和行高上下空隙）统一铺 selection.bg_fill 淡底；单元格里
                    // 的 selectable_label 一律传 false，避免在行底之上再叠一格一格
                    // 的小方块。
                    row.set_selected(row_selected);
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        if ui.checkbox(&mut batch_selected, "").changed() {
                            if batch_selected {
                                self.manuscript_selected.insert(data.id);
                            } else {
                                self.manuscript_selected.remove(&data.id);
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        let response = ui.selectable_label(
                            false,
                            egui::RichText::new(data.status.label())
                                .color(status_color(data.status)),
                        );
                        row_clicked.set(row_clicked.get() | response.clicked());
                        row_double_clicked
                            .set(row_double_clicked.get() | response.double_clicked());
                    });
                    if !compact {
                        row.col(|ui| {
                            ui.set_opacity(seen_t);
                            let response = ui.selectable_label(false, data.kind.label());
                            row_clicked.set(row_clicked.get() | response.clicked());
                            row_double_clicked
                                .set(row_double_clicked.get() | response.double_clicked());
                        });
                        row.col(|ui| {
                            ui.set_opacity(seen_t);
                            let security_level = SecurityLevel::from_marking(&data.security_level);
                            let response = ui.selectable_label(
                                false,
                                egui::RichText::new(security_level_list_label(security_level))
                                    .color(security_level_color(security_level)),
                            );
                            row_clicked.set(row_clicked.get() | response.clicked());
                            row_double_clicked
                                .set(row_double_clicked.get() | response.double_clicked());
                        });
                    }
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        let response = ui.add_sized(
                            [ui.available_width(), 20.0],
                            egui::Button::selectable(false, &data.title).truncate(),
                        );
                        row_clicked.set(row_clicked.get() | response.clicked());
                        row_double_clicked
                            .set(row_double_clicked.get() | response.double_clicked());
                    });
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        let response = ui.add_sized(
                            [ui.available_width(), 20.0],
                            egui::Button::selectable(false, &data.doc_number).truncate(),
                        );
                        row_clicked.set(row_clicked.get() | response.clicked());
                        row_double_clicked
                            .set(row_double_clicked.get() | response.double_clicked());
                    });
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        let response = ui.selectable_label(false, short_date(&data.doc_date));
                        row_clicked.set(row_clicked.get() | response.clicked());
                        row_double_clicked
                            .set(row_double_clicked.get() | response.double_clicked());
                    });
                    if !compact {
                        row.col(|ui| {
                            ui.set_opacity(seen_t);
                            let response = ui.selectable_label(false, short_date(&data.updated_at));
                            row_clicked.set(row_clicked.get() | response.clicked());
                            row_double_clicked
                                .set(row_double_clicked.get() | response.double_clicked());
                        });
                        row.col(|ui| {
                            ui.set_opacity(seen_t);
                            let response = ui.selectable_label(
                                false,
                                data.archived_at
                                    .as_deref()
                                    .map(short_date)
                                    .unwrap_or_else(|| "—".to_string()),
                            );
                            row_clicked.set(row_clicked.get() | response.clicked());
                            row_double_clicked
                                .set(row_double_clicked.get() | response.double_clicked());
                        });
                        // 已入库标记：导入前就看得出哪些进过知识库，免得同一篇
                        // 反复导入。跨来源去重虽已兜底，但让用户看见更省事。
                        // 已入库用成功色圆点 + 文字，比纯文字更易扫读。
                        row.col(|ui| {
                            ui.set_opacity(seen_t);
                            let indexed = self.knowledge_indexed_manuscripts.contains(&data.id);
                            let response = if indexed {
                                ui.horizontal(|ui| {
                                    theme::dot(ui, theme::success());
                                    ui.selectable_label(
                                        false,
                                        egui::RichText::new("已入库").color(accent()),
                                    )
                                })
                                .inner
                            } else {
                                ui.selectable_label(
                                    false,
                                    egui::RichText::new("—").color(theme::text_muted()),
                                )
                            };
                            let response = response.on_hover_text(if indexed {
                                "这篇稿件已加入知识库，再次导入会覆盖旧的索引"
                            } else {
                                "尚未加入知识库，可勾选后用工具栏的「导入到知识库」"
                            });
                            row_clicked.set(row_clicked.get() | response.clicked());
                            row_double_clicked
                                .set(row_double_clicked.get() | response.double_clicked());
                        });
                    }
                    if row_double_clicked.get() {
                        *action = Some(ManuscriptAction::Edit(data.id));
                    } else if row_clicked.get() {
                        *action = Some(ManuscriptAction::Detail(data.id));
                    }
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        if compact {
                            ui.menu_button("•••", |ui| match data.status {
                                ManuscriptStatus::Archived => {
                                    if ui.button("打开只读公文").clicked() {
                                        *action = Some(ManuscriptAction::Edit(data.id));
                                        ui.close();
                                    }
                                    if ui.button("基于此公文新建").clicked() {
                                        *action =
                                            Some(ManuscriptAction::CreateFromExisting(data.id));
                                        ui.close();
                                    }
                                }
                                ManuscriptStatus::Published => {
                                    if ui.button("打开只读公文").clicked() {
                                        *action = Some(ManuscriptAction::Edit(data.id));
                                        ui.close();
                                    }
                                    if ui.button("退回草稿").clicked() {
                                        *action = Some(ManuscriptAction::RevertToDraft(data.id));
                                        ui.close();
                                    }
                                    if ui.button("基于此公文新建").clicked() {
                                        *action =
                                            Some(ManuscriptAction::CreateFromExisting(data.id));
                                        ui.close();
                                    }
                                    if ui.button("归档").clicked() {
                                        *action = Some(ManuscriptAction::ArchivePending(data.id));
                                        ui.close();
                                    }
                                    if ui.button("删除").clicked() {
                                        *action = Some(ManuscriptAction::DeletePending(data.id));
                                        ui.close();
                                    }
                                }
                                _ => {
                                    if ui.button("打开公文").clicked() {
                                        *action = Some(ManuscriptAction::Edit(data.id));
                                        ui.close();
                                    }
                                    if ui.button("基于此公文新建").clicked() {
                                        *action =
                                            Some(ManuscriptAction::CreateFromExisting(data.id));
                                        ui.close();
                                    }
                                    if ui.button("发布").clicked() {
                                        *action = Some(ManuscriptAction::Publish(data.id));
                                        ui.close();
                                    }
                                    if ui.button("归档").clicked() {
                                        *action = Some(ManuscriptAction::ArchivePending(data.id));
                                        ui.close();
                                    }
                                    if ui.button("删除").clicked() {
                                        *action = Some(ManuscriptAction::DeletePending(data.id));
                                        ui.close();
                                    }
                                }
                            });
                            return;
                        }
                        ui.horizontal(|ui| match data.status {
                            ManuscriptStatus::Archived => {
                                if theme::icon_button(ui, theme::Icon::Eye, "查看详情")
                                    .on_hover_text("在只读公文界面中打开")
                                    .clicked()
                                {
                                    *action = Some(ManuscriptAction::Edit(data.id));
                                }
                                if theme::icon_button(ui, theme::Icon::Copy, "基于此公文新建")
                                    .on_hover_text(
                                        "复制行文要素和正文，新稿不会继承状态、版本或 PDF 附件",
                                    )
                                    .clicked()
                                {
                                    *action = Some(ManuscriptAction::CreateFromExisting(data.id));
                                }
                            }
                            ManuscriptStatus::Published => {
                                if theme::icon_button(ui, theme::Icon::Eye, "查看详情")
                                    .on_hover_text("在只读公文界面中打开")
                                    .clicked()
                                {
                                    *action = Some(ManuscriptAction::Edit(data.id));
                                }
                                if theme::icon_button(ui, theme::Icon::Undo, "退回草稿").clicked()
                                {
                                    *action = Some(ManuscriptAction::RevertToDraft(data.id));
                                }
                                if theme::icon_button(ui, theme::Icon::Copy, "基于此公文新建")
                                    .on_hover_text(
                                        "复制行文要素和正文，新稿不会继承状态、版本或 PDF 附件",
                                    )
                                    .clicked()
                                {
                                    *action = Some(ManuscriptAction::CreateFromExisting(data.id));
                                }
                                if theme::icon_button(ui, theme::Icon::Archive, "归档").clicked()
                                {
                                    *action = Some(ManuscriptAction::ArchivePending(data.id));
                                }
                                if theme::danger_icon_button(ui, theme::Icon::Trash, "删除")
                                    .clicked()
                                {
                                    *action = Some(ManuscriptAction::DeletePending(data.id));
                                }
                            }
                            _ => {
                                if theme::icon_button(ui, theme::Icon::Eye, "查看详情")
                                    .on_hover_text("打开公文编辑界面")
                                    .clicked()
                                {
                                    *action = Some(ManuscriptAction::Edit(data.id));
                                }
                                if theme::icon_button(ui, theme::Icon::Copy, "基于此公文新建")
                                    .on_hover_text(
                                        "复制行文要素和正文，新稿不会继承状态、版本或 PDF 附件",
                                    )
                                    .clicked()
                                {
                                    *action = Some(ManuscriptAction::CreateFromExisting(data.id));
                                }
                                if theme::icon_button(ui, theme::Icon::Publish, "发布").clicked()
                                {
                                    *action = Some(ManuscriptAction::Publish(data.id));
                                }
                                if theme::icon_button(ui, theme::Icon::Archive, "归档").clicked()
                                {
                                    *action = Some(ManuscriptAction::ArchivePending(data.id));
                                }
                                if theme::danger_icon_button(ui, theme::Icon::Trash, "删除")
                                    .clicked()
                                {
                                    *action = Some(ManuscriptAction::DeletePending(data.id));
                                }
                            }
                        });
                    });
                });
            });
    }

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
                        metadata_grid_row(ui, "文种", detail.kind.label());
                        metadata_grid_row(
                            ui,
                            "密级",
                            joined_metadata(&[
                                profile.security_level.as_str(),
                                profile.security_period.as_str(),
                            ])
                            .as_str(),
                        );
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
                self.manuscript_delete_confirm = Some(id);
            }
            ManuscriptAction::ArchivePending(id) => {
                self.manuscript_archive_pending = Some(ArchivePending {
                    manuscript_id: id,
                    pdf_paths: Vec::new(),
                });
            }
            ManuscriptAction::Archive(id) => {
                let pdfs = self
                    .manuscript_archive_pending
                    .take()
                    .map(|p| p.pdf_paths)
                    .unwrap_or_default();
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    store.set_status(id, ManuscriptStatus::Archived)?;
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
                    view: DiffViewState::default(),
                });
            }
            ManuscriptAction::OpenVersionDiff { manuscript_id } => {
                let latest = self.manuscript_versions.last().map(|v| v.version_number);
                self.version_diff = Some(VersionDiffState {
                    scope: VersionScope::Manuscript(manuscript_id),
                    from: latest.and_then(|n| (n > 1).then_some(n - 1)),
                    to: latest,
                    to_is_current_config: false,
                    view: DiffViewState::default(),
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
        let normalized = export::normalize_ordered_list_punctuation(&self.doc().generated_markdown);
        self.doc_mut().generated_markdown = normalized;
        // 表单刚改过而正文未动时也要在保存点重跑函号等元数据规则。
        self.draft_page().revalidate();
        let snapshot = self.doc().draft.clone();
        let content = self.doc().generated_markdown.clone();
        let title = export::extract_title(&content, &snapshot.title_hint);
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
            }
            Err(error) => self.status = format!("保存失败：{error:#}"),
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
        match doc_import::to_markdown(&path) {
            Ok(markdown) => {
                let chars = markdown.chars().count();
                let session = DraftSession::with_markdown(0, &self.config, markdown);
                self.open_doc(session);
                self.status = format!(
                    "已从 {} 新建稿件（{chars} 字）：请核对正文结构并补齐左侧公文要素。",
                    doc_import::file_label(&path)
                );
            }
            Err(error) => self.status = format!("导入失败：{error:#}"),
        }
    }

    fn open_zip_password_dialog(&mut self, purpose: ZipPasswordPurpose) {
        self.manuscript_zip_password = Some(ZipPasswordDialog::new(
            purpose,
            self.remembered_zip_password.as_deref(),
        ));
    }

    fn run_zip_password_action(
        &mut self,
        purpose: ZipPasswordPurpose,
        password: String,
        remember: bool,
    ) {
        let retry_import = match &purpose {
            ZipPasswordPurpose::Import(path) => Some(path.clone()),
            _ => None,
        };
        let completed = match purpose {
            ZipPasswordPurpose::FilteredExport => self.perform_export_manuscripts_zip(&password),
            ZipPasswordPurpose::SelectedExport => {
                self.perform_export_selected_manuscripts_zip(&password)
            }
            ZipPasswordPurpose::PdfExport(options) => {
                self.perform_export_selected_manuscript_pdfs(options, &password)
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

    fn update_remembered_zip_password(&mut self, password: Option<String>) {
        match storage::save_remembered_zip_password(password.as_deref()) {
            Ok(()) => self.remembered_zip_password = password,
            Err(error) => {
                self.status
                    .push_str(&format!(" 但未能更新记住的密码：{error:#}"));
            }
        }
    }

    pub(crate) fn export_manuscripts_zip(&mut self) {
        if self.manuscript_store.is_none() {
            self.status = "稿件库不可用，无法导出。".into();
            return;
        }
        self.open_zip_password_dialog(ZipPasswordPurpose::FilteredExport);
    }

    fn perform_export_manuscripts_zip(&mut self, password: &str) -> bool {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let default_name = format!("公文稿件-{stamp}.zip");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP 稿件包", &["zip"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return false;
        };
        let filter = self.manuscript_filter.clone();
        let result: anyhow::Result<manuscript_io::ExportSummary> = match self
            .manuscript_store
            .as_mut()
        {
            Some(store) => {
                manuscript_io::export_zip(store, &filter, &self.config.vocabulary, &path, password)
            }
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
        self.open_zip_password_dialog(ZipPasswordPurpose::SelectedExport);
    }

    fn perform_export_selected_manuscripts_zip(&mut self, password: &str) -> bool {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let default_name = format!("所选公文稿件-{stamp}.zip");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP 稿件包", &["zip"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return false;
        };
        let ids = self.manuscript_selected.iter().copied().collect::<Vec<_>>();
        let result = match self.manuscript_store.as_mut() {
            Some(store) => manuscript_io::export_zip_selected(
                store,
                &ids,
                &self.config.vocabulary,
                &path,
                password,
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
    fn perform_export_selected_manuscript_pdfs(
        &mut self,
        options: manuscript_io::PdfExportOptions,
        password: &str,
    ) -> bool {
        if self.manuscript_selected.is_empty() {
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
        let ids = self.manuscript_selected.iter().copied().collect::<Vec<_>>();
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
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
            .add_filter("ZIP 稿件包", &["zip"])
            .pick_file()
        else {
            return;
        };
        self.open_zip_password_dialog(ZipPasswordPurpose::Import(path));
    }

    fn prepare_import_manuscript(&mut self, path: PathBuf, password: &str) -> bool {
        match manuscript_io::read_manifest(&path, password) {
            Ok(manifest) => {
                // 词库读取失败不阻断稿件导入：损坏或版本不支持的词库按“不带词库”处理。
                let vocabulary = match manuscript_io::read_vocabulary(&path, password) {
                    Ok(vocabulary) => vocabulary,
                    Err(error) => {
                        self.status = format!("稿件包词库读取失败（不影响稿件导入）：{error:#}");
                        None
                    }
                };
                let selected = vec![true; manifest.records.len()];
                self.manuscript_import_preview = Some(ImportPreview {
                    manifest,
                    zip_path: path,
                    selected,
                    keyword: String::new(),
                    skip_existing: true,
                    vocabulary,
                    merge_vocabulary: true,
                    password: password.to_string(),
                });
                self.status = "已读取稿件包，请预览后确认导入。".into();
                true
            }
            Err(error) => {
                self.status = format!("读取稿件包失败：{error:#}");
                false
            }
        }
    }

    pub(crate) fn confirm_import(&mut self) {
        let Some(preview) = self.manuscript_import_preview.take() else {
            return;
        };
        let opts = manuscript_io::ImportOptions {
            skip_existing_by_id: preview.skip_existing,
            selected: preview.selected.clone(),
        };
        let result: anyhow::Result<manuscript_io::ImportSummary> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            manuscript_io::import_zip(store, &preview.zip_path, &opts, &preview.password)
        })();
        match result {
            Ok(summary) => {
                self.manuscript_dirty = true;
                let mut message = format!(
                    "已导入 {} 篇稿件、{} 个 PDF 附件。",
                    summary.imported, summary.pdfs_imported
                );
                if summary.skipped_existing > 0 {
                    message.push_str(&format!(
                        " 跳过与本地同源的 {} 篇。",
                        summary.skipped_existing
                    ));
                }
                if summary.skipped_pdfs > 0 {
                    message.push_str(&format!(
                        " {} 个附件缺失或过大被跳过。",
                        summary.skipped_pdfs
                    ));
                }
                // 勾选合并且包内带词库时，把词库增量合并进本机全局词库。
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
                self.manuscript_import_preview = Some(preview);
                self.status = format!("导入失败：{error:#}");
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
