//! 稿件列表页：筛选栏、分组确认、表格与行操作。
//!
//! 由 src/app/manuscript_ui.rs 拆分而来：本文件是模块 `app::manuscript_ui::list`，与其它子模块共享
//! `app::manuscript_ui` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::manuscript_ui::{
    ManuscriptAction, PdfExportDialog, ZipPasswordPurpose, archive_form, batch_delete_dialog,
    import_preview_form, large_dialog_size, pending_merge_form, row_delete_confirm,
    zip_password_form,
};
use crate::app::{
    FORM_CONTROL_HEIGHT, GongwenApp, accent, centered_cell_text, centered_header,
    empty_cell_placeholder, security_level_color, short_date, status_color, warn,
};
use crate::manuscript::ManuscriptFilter;
use crate::manuscript_io;
use crate::modal;
use crate::modal::Dismiss;
use crate::models::{ManuscriptStatus, SecurityLevel, TemplateKind};
use crate::theme;
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use std::collections::BTreeSet;

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
                    ui.label("文档类型");
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
                // 先为整组日期保留宽度，窄窗时整体换行，避免输入框被挤成几个字。
                ui.allocate_ui_with_layout(
                    egui::vec2(330.0, FORM_CONTROL_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
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
                            ids: self.manuscript_selected.iter().copied().collect(),
                        });
                    }
                    if ui
                        .add(theme::icon_text_button(
                            theme::Icon::PackageOpen,
                            "导入到知识库",
                        ))
                        .on_hover_text("把勾选的稿件加入知识库；再到知识库页点「建立索引」，起草时就能检索参考")
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
                        .on_hover_text("归档稿件和已归档呈批件的送批材料不会被删除")
                        .clicked()
                    {
                        self.manuscript_batch_delete = Some(self.prepare_batch_delete());
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

    /// 批量删除 / 归档 / 导入预览三组确认区。可能写入 `action`，在帧末执行。
    /// 单篇删除不在这里确认，而是在行内删除按钮下方弹出气泡，见 `delete_confirm_popup`。
    pub(crate) fn manuscript_confirm_groups(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<ManuscriptAction>,
    ) {
        if let Some(confirm) = &self.manuscript_batch_delete {
            let dialog = batch_delete_dialog(ui.ctx(), confirm);
            if dialog.inner {
                if let Some(confirm) = self.manuscript_batch_delete.take() {
                    *action = Some(ManuscriptAction::DeleteSelected(confirm.deletable));
                }
            } else if dialog.dismissed {
                self.manuscript_batch_delete = None;
            }
        }

        if let Some(export) = self.manuscript_pdf_export.as_mut() {
            let dialog = modal::dialog(
                ui.ctx(),
                egui::Id::new("manuscript_pdf_export"),
                "导出 PDF",
                400.0,
                Dismiss::EscOnly,
                |ui| {
                    ui.label(format!(
                        "已选 {} 篇稿件，导出为 PDF 压缩包：",
                        export.ids.len()
                    ));
                    ui.checkbox(&mut export.stamped, "导出盖章件（直接取附件 PDF）");
                    ui.checkbox(&mut export.compiled, "导出非盖章件（编译生成 PDF）");
                    ui.weak("文件名按稿件导出命名（不含时间戳）；盖章件名称追加（盖章）。");
                    ui.add_space(10.0);
                    let mut confirm = false;
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                export.stamped || export.compiled,
                                theme::icon_text_button(theme::Icon::FileDown, "导出"),
                            )
                            .clicked()
                        {
                            confirm = true;
                        }
                        if ui.button("取消").clicked() {
                            ui.close();
                        }
                    });
                    confirm
                },
            );
            if dialog.inner {
                if let Some(export) = self.manuscript_pdf_export.take() {
                    let options = manuscript_io::PdfExportOptions {
                        stamped: export.stamped,
                        compiled: export.compiled,
                    };
                    self.open_zip_password_dialog(ZipPasswordPurpose::PdfExport(
                        options, export.ids,
                    ));
                }
            } else if dialog.dismissed {
                self.manuscript_pdf_export = None;
            }
        }

        if let Some(password) = self.manuscript_zip_password.as_mut() {
            let importing = password.purpose.is_import();
            // 有输入的表单：点遮罩不关，免得误点丢掉已经敲好的密码；Esc 与「取消」照常关。
            let dialog = modal::dialog(
                ui.ctx(),
                egui::Id::new("manuscript_zip_password"),
                if importing {
                    "输入 ZIP 密码"
                } else {
                    "设置 ZIP 加密密码"
                },
                420.0,
                Dismiss::EscOnly,
                |ui| zip_password_form(ui, password, importing),
            );
            if dialog.inner {
                self.submit_zip_password();
            } else if dialog.dismissed {
                self.manuscript_zip_password = None;
            }
        }

        if let Some(pending) = self.manuscript_archive_pending.as_mut() {
            let manuscript_id = pending.manuscript_id;
            let title = self
                .manuscript_rows
                .iter()
                .find(|row| row.id == manuscript_id)
                .map(|row| row.title.clone())
                .unwrap_or_default();
            // 选了附件、选了钉版方式的表单：点遮罩不关，Esc 与「取消」照常关。
            let dialog = modal::dialog(
                ui.ctx(),
                egui::Id::new("manuscript_archive"),
                "归档稿件",
                460.0,
                Dismiss::EscOnly,
                |ui| archive_form(ui, pending, &title),
            );
            if dialog.inner {
                // 保留 pending（含已选 PDF），由 Archive action 读取并执行归档。
                *action = Some(ManuscriptAction::Archive(manuscript_id));
            } else if dialog.dismissed {
                self.manuscript_archive_pending = None;
            }
        }

        if let Some(preview) = self.manuscript_import_preview.as_mut() {
            let (width, body) = large_dialog_size(ui.ctx());
            // 逐篇选好的处理方式重来一遍要再输一次密码，只认按钮，Esc、遮罩都不关。
            let dialog = modal::dialog(
                ui.ctx(),
                egui::Id::new("manuscript_import_preview"),
                "导入预览",
                width,
                Dismiss::ButtonsOnly,
                |ui| import_preview_form(ui, preview, &self.config, body),
            );
            match dialog.inner {
                Some(true) => self.confirm_import(),
                Some(false) => self.manuscript_import_preview = None,
                None => {}
            }
        }

        if let Some(merge) = self.pending_merge.as_mut() {
            let (width, body) = large_dialog_size(ui.ctx());
            // 「稍后处理」只是关框，分支仍暂存在库里，Esc 同样处理；点遮罩不关。
            let dialog = modal::dialog(
                ui.ctx(),
                egui::Id::new("manuscript_pending_merge"),
                "继续合并待处理分支",
                width,
                Dismiss::EscOnly,
                |ui| pending_merge_form(ui, merge, &self.config, body),
            );
            if dialog.inner {
                self.confirm_pending_merge();
            } else if dialog.dismissed {
                self.pending_merge = None;
            }
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
            .striped(!theme::smartisan::active())
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
                    theme::smartisan::ledger_header(ui);
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
                    theme::smartisan::ledger_header(ui);
                    centered_header(ui, "状态");
                });
                if !compact {
                    header.col(|ui| {
                        theme::smartisan::ledger_header(ui);
                        centered_header(ui, "文档类型");
                    });
                    header.col(|ui| {
                        theme::smartisan::ledger_header(ui);
                        centered_header(ui, "密级");
                    });
                }
                header.col(|ui| {
                    theme::smartisan::ledger_header(ui);
                    centered_header(ui, "标题");
                });
                header.col(|ui| {
                    theme::smartisan::ledger_header(ui);
                    centered_header(ui, "文号");
                });
                header.col(|ui| {
                    theme::smartisan::ledger_header(ui);
                    centered_header(ui, "成文日期");
                });
                if !compact {
                    header.col(|ui| {
                        theme::smartisan::ledger_header(ui);
                        centered_header(ui, "更新");
                    });
                    header.col(|ui| {
                        theme::smartisan::ledger_header(ui);
                        centered_header(ui, "归档");
                    });
                    header.col(|ui| {
                        theme::smartisan::ledger_header(ui);
                        centered_header(ui, "知识库");
                    });
                }
                header.col(|ui| {
                    theme::smartisan::ledger_header(ui);
                    ui.strong(if compact { "更多" } else { "操作" });
                });
                // 表头悬停提示：列宽其实可拖拽调整，但界面上没有任何暗示，
                // 用户会以为列宽是写死的。response() 是所有表头单元格的并集。
                header
                    .response()
                    .on_hover_text("拖动列头可调整列宽（操作列除外）");
            })
            .body(|mut body| {
                let ledger_painter = body.ui_mut().painter().clone();
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
                        let response = centered_cell_text(
                            ui,
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
                            let response = centered_cell_text(ui, data.kind.label());
                            row_clicked.set(row_clicked.get() | response.clicked());
                            row_double_clicked
                                .set(row_double_clicked.get() | response.double_clicked());
                        });
                        row.col(|ui| {
                            ui.set_opacity(seen_t);
                            let security_level = SecurityLevel::from_marking(&data.security_level);
                            let response = if security_level == SecurityLevel::Unmarked {
                                empty_cell_placeholder(ui).on_hover_text("未标密")
                            } else {
                                centered_cell_text(
                                    ui,
                                    egui::RichText::new(security_level.marking())
                                        .color(security_level_color(security_level)),
                                )
                            };
                            row_clicked.set(row_clicked.get() | response.clicked());
                            row_double_clicked
                                .set(row_double_clicked.get() | response.double_clicked());
                        });
                    }
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        let response = centered_cell_text(ui, &data.title);
                        row_clicked.set(row_clicked.get() | response.clicked());
                        row_double_clicked
                            .set(row_double_clicked.get() | response.double_clicked());
                    });
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        let response = if data.doc_number.trim().is_empty() {
                            empty_cell_placeholder(ui).on_hover_text("未编文号")
                        } else {
                            centered_cell_text(ui, &data.doc_number)
                        };
                        row_clicked.set(row_clicked.get() | response.clicked());
                        row_double_clicked
                            .set(row_double_clicked.get() | response.double_clicked());
                    });
                    row.col(|ui| {
                        ui.set_opacity(seen_t);
                        let response = if data.doc_date.trim().is_empty() {
                            empty_cell_placeholder(ui).on_hover_text("未填成文日期")
                        } else {
                            centered_cell_text(ui, short_date(&data.doc_date))
                        };
                        row_clicked.set(row_clicked.get() | response.clicked());
                        row_double_clicked
                            .set(row_double_clicked.get() | response.double_clicked());
                    });
                    if !compact {
                        row.col(|ui| {
                            ui.set_opacity(seen_t);
                            let response = centered_cell_text(ui, short_date(&data.updated_at));
                            row_clicked.set(row_clicked.get() | response.clicked());
                            row_double_clicked
                                .set(row_double_clicked.get() | response.double_clicked());
                        });
                        row.col(|ui| {
                            ui.set_opacity(seen_t);
                            let response = match data.archived_at.as_deref() {
                                Some(at) if !at.trim().is_empty() => {
                                    centered_cell_text(ui, short_date(at))
                                }
                                _ => empty_cell_placeholder(ui),
                            };
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
                                // 圆点 + 文字是两个部件，没法交给 centered_cell_text；
                                // 先量出整组宽度，左侧补足一半余量，让这组内容落在列中线上。
                                let label = egui::RichText::new("已入库").color(accent());
                                let text_width = egui::WidgetText::from(label.clone())
                                    .into_galley(
                                        ui,
                                        Some(egui::TextWrapMode::Extend),
                                        f32::INFINITY,
                                        egui::TextStyle::Button,
                                    )
                                    .size()
                                    .x;
                                let spacing = ui.spacing();
                                let group_width = 8.0
                                    + spacing.item_spacing.x
                                    + text_width
                                    + 2.0 * spacing.button_padding.x;
                                ui.add_space(((ui.available_width() - group_width) / 2.0).max(0.0));
                                theme::dot(ui, theme::success());
                                ui.selectable_label(false, label)
                            } else {
                                empty_cell_placeholder(ui)
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
                            let menu = ui.menu_button("•••", |ui| match data.status {
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
                            // 菜单点「删除」后自身已收起，确认气泡改贴在「•••」下方。
                            row_delete_confirm(
                                &menu.response,
                                &mut self.manuscript_delete_confirm,
                                &self.manuscript_delete_refs,
                                data.id,
                                action,
                            );
                            return;
                        }
                        let mut trash_button = None;
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
                                let trash =
                                    theme::danger_icon_button(ui, theme::Icon::Trash, "删除");
                                if trash.clicked() {
                                    // 再点一次删除按钮就收起确认气泡。
                                    if self.manuscript_delete_confirm == Some(data.id) {
                                        self.manuscript_delete_confirm = None;
                                    } else {
                                        *action = Some(ManuscriptAction::DeletePending(data.id));
                                    }
                                }
                                trash_button = Some(trash);
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
                                let trash =
                                    theme::danger_icon_button(ui, theme::Icon::Trash, "删除");
                                if trash.clicked() {
                                    // 再点一次删除按钮就收起确认气泡。
                                    if self.manuscript_delete_confirm == Some(data.id) {
                                        self.manuscript_delete_confirm = None;
                                    } else {
                                        *action = Some(ManuscriptAction::DeletePending(data.id));
                                    }
                                }
                                trash_button = Some(trash);
                            }
                        });
                        if let Some(trash) = trash_button {
                            row_delete_confirm(
                                &trash,
                                &mut self.manuscript_delete_confirm,
                                &self.manuscript_delete_refs,
                                data.id,
                                action,
                            );
                        }
                    });
                    theme::smartisan::index_row(&ledger_painter, row.response().rect, row_selected);
                });
            });
    }
}
