//! 稿件管理中的销毁审批表预览、确认和后台导出。

use crate::app::file_dialog::FileDialogSpec;
use crate::app::manuscript_ui::large_dialog_size;
use crate::app::{GongwenApp, WorkerResult, centered_cell_text, centered_header, warn};
use crate::export::protection::{self, Protection};
use crate::manuscript::ManuscriptStore;
use crate::manuscript::disposal::{DisposalBatch, FormKind};
use crate::modal::{self, Dismiss};
use crate::storage;
use anyhow::{Context, Result, ensure};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

pub(crate) enum DisposalDialog {
    Preparing,
    Review(DisposalReview),
    Exporting,
}

pub(crate) struct DisposalReview {
    batch: DisposalBatch,
    file: bool,
    disc: bool,
    active_form: FormKind,
    notice: Option<String>,
}

impl GongwenApp {
    pub(crate) fn disposal_forms_busy(&self) -> bool {
        matches!(
            self.manuscript_disposal,
            Some(DisposalDialog::Preparing | DisposalDialog::Exporting)
        )
    }

    pub(super) fn prepare_disposal_forms(&mut self, ctx: &egui::Context) {
        let ids: Vec<_> = self
            .manuscript_rows
            .iter()
            .filter(|row| self.manuscript_selected.contains(&row.id))
            .map(|row| row.id)
            .collect();
        let db_path = match storage::manuscript_db_path() {
            Ok(path) => path,
            Err(error) => {
                self.status = format!("无法打开稿件库：{error:#}");
                return;
            }
        };
        self.manuscript_disposal = Some(DisposalDialog::Preparing);
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = ManuscriptStore::open(&db_path)
                .and_then(|store| store.disposal_batch(&ids))
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::DisposalFormsPrepared(result));
            ctx.request_repaint();
        });
    }

    pub(crate) fn disposal_forms_prepared(&mut self, result: Result<DisposalBatch, String>) {
        self.manuscript_disposal = match result {
            Ok(batch) => Some(DisposalDialog::Review(DisposalReview {
                batch,
                file: true,
                disc: true,
                active_form: FormKind::File,
                notice: None,
            })),
            Err(error) => {
                self.status = format!("读取销毁审批表数据失败：{error}");
                None
            }
        };
    }

    pub(crate) fn disposal_forms_window(&mut self, ctx: &egui::Context) {
        let can_act = self.pending_file_dialogs.is_empty() && self.pending_output.is_none();
        let Some(dialog) = self.manuscript_disposal.as_mut() else {
            return;
        };
        let id = egui::Id::unique("manuscript_disposal_forms");
        let DisposalDialog::Review(review) = dialog else {
            modal::dialog(
                ctx,
                id,
                "导出销毁审批表",
                360.0,
                Dismiss::ButtonsOnly,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(match dialog {
                            DisposalDialog::Preparing => "正在读取所选稿件…",
                            _ => "正在生成 Word 审批表…",
                        });
                    });
                },
            );
            return;
        };
        let (width, height) = large_dialog_size(ctx);
        let response = modal::dialog(
            ctx,
            id,
            "导出销毁审批表",
            width,
            Dismiss::EscOnly,
            |ui| {
                ui.add_enabled_ui(can_act, |ui| review.show(ui, height))
                    .inner
            },
        );
        if response.dismissed && can_act {
            self.manuscript_disposal = None;
        } else if response.inner {
            self.choose_disposal_output();
        }
    }

    fn choose_disposal_output(&mut self) {
        self.request_document_output("导出销毁审批表（Word）", |app| {
            let settings = Protection::current()
                .and_then(|protection| Ok((protection, storage::manuscript_db_path()?)));
            let (protection, db_path) = match settings {
                Ok(settings) => settings,
                Err(error) => {
                    if let Some(DisposalDialog::Review(review)) = app.manuscript_disposal.as_mut() {
                        review.notice = Some(format!("无法导出：{error:#}"));
                    }
                    return;
                }
            };
            app.request_file_with_context(FileDialogSpec::pick_folder(), move |app, path, ctx| {
                let Some(directory) = path else { return };
                let Some(DisposalDialog::Review(mut review)) = app.manuscript_disposal.take()
                else {
                    return;
                };
                app.manuscript_disposal = Some(DisposalDialog::Exporting);
                let tx = app.sender.clone();
                std::thread::spawn(move || {
                    let result = review
                        .export(&db_path, &directory, protection.as_ref())
                        .map_err(|error| (review, format!("{error:#}")));
                    let _ = tx.send(WorkerResult::DisposalFormsExported(result));
                    ctx.request_repaint();
                });
            });
        });
        if self.pending_output.is_none()
            && let Some(DisposalDialog::Review(review)) = self.manuscript_disposal.as_mut()
        {
            review.notice = Some(self.status.clone());
        }
    }

    pub(crate) fn disposal_forms_exported(
        &mut self,
        result: Result<PathBuf, (DisposalReview, String)>,
    ) {
        self.manuscript_disposal = match result {
            Ok(directory) => {
                self.status = format!("销毁审批表已导出至 {}", directory.display());
                None
            }
            Err((mut review, error)) => {
                review.notice = Some(format!("导出失败：{error}"));
                Some(DisposalDialog::Review(review))
            }
        };
    }
}

impl DisposalReview {
    fn show(&mut self, ui: &mut egui::Ui, height: f32) -> bool {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.file, FormKind::File.title());
            ui.checkbox(&mut self.disc, FormKind::Disc.title());
            ui.weak(format!("已选 {} 篇稿件", self.batch.files.len()));
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.active_form, FormKind::File, "文件表预览");
            ui.selectable_value(&mut self.active_form, FormKind::Disc, "光盘表预览");
        });
        let numbers = if self.disc || self.active_form == FormKind::Disc {
            self.batch.planned_disc_numbers()
        } else {
            Ok(Vec::new())
        };
        ui.weak(match self.active_form {
            FormKind::File => "公函按版记计算份数，其余默认 1 份。缺少文件号或密级时可补填或留空。",
            FormKind::Disc => "一盘一稿，每盘 1 份。编号留空时自动分配，也可填写盘上已有的编号。",
        });
        self.show_table(ui, height, numbers.as_deref().unwrap_or_default());
        if let Err(error) = &numbers {
            ui.colored_label(warn(), error.to_string());
        }
        if let Some(notice) = &self.notice {
            ui.colored_label(warn(), notice);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("取消").clicked() {
                ui.close();
            }
            ui.add_enabled(
                (self.file || self.disc) && (!self.disc || numbers.is_ok()),
                egui::Button::new("选择目录并导出"),
            )
            .clicked()
        })
        .inner
    }

    fn show_table(&mut self, ui: &mut egui::Ui, height: f32, disc_numbers: &[String]) {
        let kind = self.active_form;
        let entries = self.batch.rows_mut(kind);
        let row_height = ui.spacing().interact_size.y + 6.0;
        TableBuilder::new(ui)
            .id_salt(("disposal_form", kind == FormKind::Disc))
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .min_scrolled_height(0.0)
            .max_scroll_height(height)
            .column(Column::exact(42.0))
            .column(Column::initial(136.0).at_least(106.0).resizable(true))
            .column(Column::remainder().at_least(132.0))
            .column(Column::exact(74.0))
            .column(Column::exact(58.0))
            .header(row_height, |mut header| {
                for label in kind.columns() {
                    header.col(|ui| {
                        centered_header(ui, label);
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, entries.len(), |mut row| {
                    let index = row.index();
                    let entry = &mut entries[index];
                    let number_hint = match kind {
                        FormKind::File => "未填",
                        FormKind::Disc => disc_numbers.get(index).map_or("自动", String::as_str),
                    };
                    row.col(|ui| {
                        centered_cell_text(ui, (index + 1).to_string());
                    });
                    row.col(|ui| edit_cell(ui, &mut entry.number, number_hint));
                    row.col(|ui| edit_cell(ui, &mut entry.title, ""));
                    row.col(|ui| edit_cell(ui, &mut entry.security, "未填"));
                    row.col(|ui| {
                        if kind == FormKind::File {
                            ui.add(egui::DragValue::new(&mut entry.copies).range(1..=u32::MAX));
                        } else {
                            centered_cell_text(ui, "1");
                        }
                    });
                });
            });
    }

    fn export(
        &mut self,
        db_path: &Path,
        directory: &Path,
        protection: Option<&Protection>,
    ) -> Result<PathBuf> {
        ensure!(!storage::writes_blocked(), "工作区正在锁定，已取消导出");
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let output = tempfile::Builder::new()
            .prefix(&format!("销毁审批表-{stamp}-"))
            .rand_bytes(4)
            .tempdir_in(directory)
            .context("创建审批表输出目录失败")?;
        // 编号先在库内事务中预留。磁盘写失败时保留在预览里，重试沿用原编号。
        if self.disc {
            ManuscriptStore::open(db_path)?.reserve_disposal_disc_numbers(&mut self.batch)?;
        }
        for kind in FormKind::ALL {
            let selected = match kind {
                FormKind::File => self.file,
                FormKind::Disc => self.disc,
            };
            if selected {
                let path = output.path().join(format!("{}.docx", kind.title()));
                let docx = Zeroizing::new(crate::export::build_disposal_docx(
                    kind,
                    self.batch.rows(kind),
                )?);
                protection::write_final_with(protection, &path, &docx)
                    .with_context(|| format!("无法保存《{}》", kind.title()))?;
            }
        }
        ensure!(!storage::writes_blocked(), "工作区正在锁定，已取消导出");
        Ok(output.keep())
    }
}

fn edit_cell(ui: &mut egui::Ui, value: &mut String, hint: &str) {
    let edit = ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(ui.available_width()),
    );
    if !value.is_empty() {
        edit.on_hover_text(value.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::NewManuscript;

    #[test]
    fn disposal_export_writes_only_selected_forms_and_numbers_only_discs() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("drafts.db");
        let mut store = ManuscriptStore::open(&db).unwrap();
        let id = store
            .create(
                &NewManuscript {
                    content_markdown: "# 工作通知\n\n正文".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let mut review = DisposalReview {
            batch: store.disposal_batch(&[id]).unwrap(),
            file: true,
            disc: false,
            active_form: FormKind::File,
            notice: None,
        };
        let first = review.export(&db, directory.path(), None).unwrap();
        assert!(first.join("文件销毁审批表.docx").is_file());
        assert!(!first.join("光盘销毁审批表.docx").exists());
        assert_eq!(store.disposal_batch(&[id]).unwrap().last_disc_serial, 0);

        review.disc = true;
        let both = review.export(&db, directory.path(), None).unwrap();
        assert_ne!(first, both);
        assert!(both.join("文件销毁审批表.docx").is_file());
        assert!(both.join("光盘销毁审批表.docx").is_file());
        assert_eq!(review.batch.discs[0].number, "GCB-0001");
        assert_eq!(store.disposal_batch(&[id]).unwrap().last_disc_serial, 1);
    }
}
