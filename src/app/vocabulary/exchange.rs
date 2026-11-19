//! 标准词库 Excel 的文件、介质交付与回导。
use crate::app::file_dialog::FileDialogSpec;
use crate::app::jobs::WorkerResult;
use crate::app::media_export::{Medium, require_workspace};
use crate::app::{GongwenApp, warn};
use crate::export::protection::{self, Protection, office};
use crate::modal::{self, Dismiss};
use crate::models::{VocabularyEntry, VocabularySetupStatus};
use crate::storage::{self, DataSlot};
use crate::vault::KeyHandle;
use crate::vocabulary_xlsx::{self, ImportError, ImportReport};
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use zeroize::Zeroizing;

pub(in crate::app) struct VocabularyImport {
    name: String,
    slot: DataSlot,
    unlock: Option<KeyHandle>,
    encrypted: Option<Arc<[u8]>>,
    password: Zeroizing<String>,
    error: String,
    pending: Option<mpsc::Receiver<LoadedWorkbook>>,
}

enum LoadedWorkbook {
    NeedsPassword(Arc<[u8]>),
    InvalidPasswordOrFile(String),
    Parsed(Result<ImportReport, ImportError>),
}

impl VocabularyImport {
    pub(in crate::app) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn same_workspace(&self) -> bool {
        let same_unlock = match (&self.unlock, crate::vault::key_handle()) {
            (Some(expected), Some(current)) => expected.same_unlock(&current),
            (None, None) => true,
            _ => false,
        };
        same_unlock && self.slot == storage::current_slot() && !storage::writes_blocked()
    }
}

enum WorkbookDestination {
    File(PathBuf, Option<Protection>),
    Disc(Medium, KeyHandle),
}

impl GongwenApp {
    pub(crate) fn import_vocabulary_xlsx(&mut self) {
        if self.vocabulary_import.is_some() {
            self.status = "已有标准词库正在导入。".into();
            return;
        }
        let slot = storage::current_slot();
        let unlock = crate::vault::key_handle();
        self.request_file_with_context(
            FileDialogSpec::pick_file().filter("Excel 词库", &["xlsx"]),
            move |app, path, ctx| {
                let Some(path) = path else { return };
                let mut state = VocabularyImport {
                    name: path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Excel 词库")
                        .into(),
                    slot,
                    unlock,
                    encrypted: None,
                    password: Default::default(),
                    error: String::new(),
                    pending: None,
                };
                if !state.same_workspace() || app.vault_locked || app.vocabulary_import.is_some() {
                    app.status = "工作区已变化或已有导入任务，已取消本次导入。".into();
                    return;
                }
                let codes = vocabulary_xlsx::unit_codes(&app.config.vocabulary);
                state.pending = Some(load_workbook(
                    ctx,
                    move || match crate::file_limits::read_bytes(
                        &path,
                        crate::file_limits::MAX_DOCUMENT_BYTES,
                        "Excel 词库",
                    ) {
                        Ok(bytes) if bytes.starts_with(&office::COMPOUND_MAGIC) => {
                            LoadedWorkbook::NeedsPassword(bytes.into())
                        }
                        Ok(bytes) => {
                            LoadedWorkbook::Parsed(vocabulary_xlsx::parse_bytes(&bytes, &codes))
                        }
                        Err(error) => LoadedWorkbook::Parsed(Err(ImportError {
                            conflicts: Vec::new(),
                            summary: format!("读取 Excel 词库失败：{error}"),
                        })),
                    },
                ));
                app.vocabulary_import_conflicts = None;
                app.vocabulary_import = Some(state);
                app.status = "正在读取 Excel 词库…".into();
            },
        );
    }

    pub(in crate::app) fn vocabulary_import_window(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.vocabulary_import.take() else {
            return;
        };
        if !state.same_workspace() || self.vault_locked {
            self.status = "工作区已变化，已取消词库导入。".into();
            return;
        }
        if let Some(pending) = &state.pending {
            match pending.try_recv() {
                Ok(LoadedWorkbook::NeedsPassword(bytes)) => state.encrypted = Some(bytes),
                Ok(LoadedWorkbook::InvalidPasswordOrFile(error)) => {
                    state.error = error;
                    self.status = "等待核对工作簿密码。".into();
                }
                Ok(LoadedWorkbook::Parsed(result)) => {
                    self.apply_vocabulary_workbook(&state.name, result);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.status = "词库读取任务已退出。".into();
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.vocabulary_import = Some(state);
                    return;
                }
            }
            state.pending = None;
        }

        let mut cancel = false;
        let mut confirm = false;
        let modal = modal::dialog(
            ctx,
            egui::Id::unique("vocabulary_workbook_password"),
            "输入 Excel 工作簿密码",
            440.0,
            Dismiss::EscOnly,
            |ui| {
                ui.label(&state.name);
                let password = crate::ime::exempt(
                    ui.add(egui::TextEdit::singleline(&mut *state.password).password(true)),
                );
                confirm = password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if !state.error.is_empty() {
                    ui.colored_label(warn(), &state.error);
                }
                ui.horizontal(|ui| {
                    cancel = ui.button("取消").clicked();
                    confirm |= ui
                        .add_enabled(!state.password.is_empty(), egui::Button::new("解密并导入"))
                        .clicked();
                });
            },
        );
        if cancel || modal.dismissed {
            self.status = "已取消词库导入。".into();
            return;
        }
        if confirm
            && !state.password.is_empty()
            && let Some(bytes) = &state.encrypted
        {
            let bytes = Arc::clone(bytes);
            let password = std::mem::take(&mut state.password);
            let codes = vocabulary_xlsx::unit_codes(&self.config.vocabulary);
            state.pending = Some(load_workbook(ctx.clone(), move || {
                match office::decrypt(&bytes, &password) {
                    Ok(plaintext) => {
                        LoadedWorkbook::Parsed(vocabulary_xlsx::parse_bytes(&plaintext, &codes))
                    }
                    Err(error) => LoadedWorkbook::InvalidPasswordOrFile(format!("{error:#}")),
                }
            }));
            state.error.clear();
            self.status = "正在解密并校验工作簿…".into();
        }
        self.vocabulary_import = Some(state);
    }

    fn apply_vocabulary_workbook(&mut self, name: &str, result: Result<ImportReport, ImportError>) {
        let parsed = match result {
            Ok(parsed) => parsed,
            Err(error) => {
                if !error.conflicts.is_empty() {
                    self.vocabulary_import_conflicts = Some(error.conflicts);
                }
                self.status = format!("词库导入失败：{}", error.summary);
                return;
            }
        };
        let count = parsed.entries.len();
        let mut staged_config = self.config.clone();
        let result = (|| -> Result<vocabulary_xlsx::MergeReport, String> {
            let merge = vocabulary_xlsx::merge(&mut staged_config.vocabulary, parsed.entries)?;
            crate::units::normalize(&mut staged_config.vocabulary);
            if !staged_config.vocabulary.is_empty() {
                staged_config.vocabulary_setup = VocabularySetupStatus::Completed;
            }
            storage::save(&staged_config).map_err(|error| format!("保存词库失败：{error:#}"))?;
            Ok(merge)
        })();
        match result {
            Ok(merge) => {
                self.config = staged_config;
                self.vocabulary_dirty = false;
                self.status = format!(
                    "已从“{name}”解析 {count} 条：新增 {} 条、更新 {} 条、未变化 {} 条。",
                    merge.added, merge.updated, merge.unchanged
                );
            }
            Err(error) => self.status = format!("词库导入失败：{error}"),
        }
    }

    pub(super) fn export_vocabulary_xlsx(&mut self, ctx: &egui::Context, blank: bool) {
        let (filename, operation) = if blank {
            (
                "公文助手标准词库模板.xlsx",
                "导出空白词库模板（含单位编码下拉）",
            )
        } else {
            (
                "公文助手标准词库.xlsx",
                "导出标准词库 Excel（含姓名、职务、联系电话、个人简介）",
            )
        };
        let vocabulary = self.config.vocabulary.clone();
        let file_vocabulary = vocabulary.clone();
        let save = Box::new(move |app: &mut Self| {
            let protection = match Protection::current() {
                Ok(protection) => protection,
                Err(error) => {
                    app.status = format!("无法导出：{error:#}");
                    return;
                }
            };
            app.request_file_with_context(
                FileDialogSpec::save_file()
                    .filter("Excel", &["xlsx"])
                    .file_name(filename),
                move |app, path, ctx| {
                    if let Some(path) = path {
                        app.start_vocabulary_workbook_export(
                            ctx,
                            file_vocabulary,
                            filename,
                            blank,
                            WorkbookDestination::File(path.with_extension("xlsx"), protection),
                        );
                    }
                },
            );
        });
        if protection::encryption_enabled() {
            self.choose_document_delivery(
                ctx,
                "导出密码加密 Excel…",
                operation.into(),
                save,
                move |app, medium, key, ctx| {
                    app.start_vocabulary_workbook_export(
                        ctx,
                        vocabulary,
                        filename,
                        blank,
                        WorkbookDestination::Disc(medium, key),
                    );
                },
            );
        } else {
            self.request_output_with_warning(
                operation,
                crate::app::output_guard::PII_EXPORT_WARNING,
                save,
            );
        }
    }

    fn start_vocabulary_workbook_export(
        &mut self,
        ctx: egui::Context,
        vocabulary: Vec<VocabularyEntry>,
        filename: &'static str,
        blank: bool,
        target: WorkbookDestination,
    ) {
        if self.vocabulary_export_busy {
            self.status = "已有词库工作簿正在导出。".into();
            return;
        }
        self.vocabulary_export_busy = true;
        self.status = "正在生成 Excel 工作簿…".into();
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let result = (|| {
                if let WorkbookDestination::Disc(_, key) = &target {
                    require_workspace(key)?;
                }
                let entries = if blank { &[][..] } else { &vocabulary[..] };
                let mut workbook = Zeroizing::new(vocabulary_xlsx::xlsx_bytes(
                    entries,
                    &vocabulary_xlsx::unit_codes(&vocabulary),
                )?);
                match target {
                    WorkbookDestination::File(path, protection) => {
                        let path =
                            protection::write_final_with(protection.as_ref(), &path, &workbook)?;
                        Ok::<_, anyhow::Error>(format!("已导出 Excel 到 {}。", path.display()))
                    }
                    WorkbookDestination::Disc(medium, key) => {
                        let files = [(filename.to_owned(), std::mem::take(&mut *workbook))].into();
                        let image = Zeroizing::new(crate::optical_image::build(files)?);
                        medium.burn(&key, &image)?;
                        Ok(format!("Excel 已刻录并回读校验：{}。", medium.label()))
                    }
                }
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::VocabularyWorkbookExported(result));
            ctx.request_repaint();
        });
    }
}

fn load_workbook(
    ctx: egui::Context,
    load: impl FnOnce() -> LoadedWorkbook + Send + 'static,
) -> mpsc::Receiver<LoadedWorkbook> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(load());
        ctx.request_repaint();
    });
    rx
}
