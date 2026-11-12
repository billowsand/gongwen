//! 数据安全相关的界面：启动时的配置 / 崩溃日志提示，以及设置页的一键备份与恢复。
//!
//! 三个提示都走 [`crate::modal`] 的独占模态框：启动提示只弹一次，备份恢复在
//! 执行期间挡住其他操作，避免用户一边恢复一边编辑。
//! 备份文件的读写与回滚在 [`crate::data_backup`]，这里只负责交互与状态。

use super::file_dialog::FileDialogSpec;
use super::settings::{setting_row, sub_heading};
use super::{GongwenApp, WorkerResult, open_in_os, warn};
use crate::data_backup;
#[cfg(feature = "ai-workbench")]
use crate::knowledge::KnowledgeStore;
use crate::lexicon::LexiconStore;
use crate::manuscript::ManuscriptStore;
use crate::modal::{self, Dismiss};
use crate::storage;
use crate::theme;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::thread;

impl GongwenApp {
    /// 每帧末尾调用：损坏配置、崩溃日志与备份恢复的全局提示。
    pub(crate) fn data_safety_windows(&mut self, ctx: &egui::Context) {
        self.config_recovery_window(ctx);
        // 两个启动提示不同时叠着弹：先让用户看完配置损坏这件事，关掉后再提示崩溃日志。
        if self.config_recovered_from.is_none() {
            self.crash_report_window(ctx);
        }
        self.backup_restore_confirm_window(ctx);
        self.backup_restoring_window(ctx);
        self.backup_restore_done_window(ctx);
        self.backup_restore_error_window(ctx);
        self.content_clear_confirm_window(ctx);
    }

    fn config_recovery_window(&mut self, ctx: &egui::Context) {
        config_recovery_dialog(ctx, &mut self.config_recovered_from);
    }

    fn crash_report_window(&mut self, ctx: &egui::Context) {
        if let Some(error) = crash_report_dialog(ctx, &mut self.crash_report_pending) {
            self.status = error;
        }
    }

    /// 设置页「数据备份」分区。
    pub(crate) fn backup_section_ui(&mut self, ui: &mut egui::Ui) {
        sub_heading(
            ui,
            "全量备份",
            Some(
                "把配置目录里的有效数据打成一个内容和文件名均加密的 7z，换机器或重装系统前先导出一份。",
            ),
        );
        let backup_contents = if cfg!(feature = "ai-workbench") {
            "包含配置、稿件库、知识库、图片、数据接口、输入法词表、技能和 Pi 运行数据。崩溃日志等本机诊断文件不打包。"
        } else {
            "包含配置、稿件库、图片和输入法词表。"
        };
        let export_tip = format!(
            "选一个配置目录之外的位置保存加密 7z。\n{backup_contents}\n{}",
            concat!(
                "导出前会保存已修改的稿件。备份完成后，状态栏会显示文件大小。\n",
                "备份期间请保持应用开启，避免修改应用锁设置。",
            )
        );
        let mut restore_tip = "先校验备份文件，再确认是否恢复。".to_owned();
        if let Ok(dir) = storage::config_dir() {
            restore_tip.push_str(&format!(
                "\n恢复前会把当前数据留底到：{}",
                data_backup::restore_backup_location(&dir).display(),
            ));
        }

        let mut export = false;
        let mut inspect = false;
        let idle = !self.any_busy();
        ui.add_enabled_ui(idle, |ui| {
            setting_row(ui, "导出备份", None, |ui| {
                export = theme::primary_icon_button(ui, theme::Icon::Archive, "导出备份…")
                    .on_hover_text(export_tip)
                    .clicked();
            });
            setting_row(ui, "恢复备份", None, |ui| {
                inspect = ui
                    .add(theme::secondary_icon_button(
                        theme::Icon::RotateCcw,
                        "恢复备份…",
                    ))
                    .on_hover_text(restore_tip)
                    .clicked();
            });
        });
        if self.backup_busy {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.weak("正在处理备份…")
                    .on_hover_text("导出期间应用锁设置发生变化会取消本次备份。");
            });
        } else if !idle {
            ui.add_space(6.0);
            ui.weak("有其他任务正在运行，请完成后再导出或恢复备份。");
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        sub_heading(
            ui,
            "清空数据",
            Some("永久删除标准词库、全部文稿、资料库与维基库；应用设置和任务助手数据保留。"),
        );
        ui.add_enabled_ui(idle, |ui| {
            setting_row(ui, "清空内容", None, |ui| {
                if ui
                    .add(theme::warning_icon_button(theme::Icon::Trash, "清空数据…"))
                    .on_hover_text(
                        "永久删除词条、稿件（含归档、版本、附件和插图）、资料库和维基库。",
                    )
                    .clicked()
                {
                    self.content_clear_confirm = true;
                }
            });
        });
        if export {
            self.start_backup_export();
        }
        if inspect {
            self.start_backup_inspection();
        }
    }

    fn content_clear_confirm_window(&mut self, ctx: &egui::Context) {
        if !self.content_clear_confirm {
            return;
        }
        let mut confirm = false;
        let mut cancel = false;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("content_clear_confirm"),
            "清空数据",
            500.0,
            Dismiss::EscOrBackdrop,
            |ui| {
                ui.label(
                    egui::RichText::new("以下内容将被永久删除，无法撤销：")
                        .strong()
                        .color(warn()),
                );
                ui.add_space(6.0);
                ui.label("• 标准词库中的全部词条");
                ui.label("• 全部文稿，包括归档稿件、历史版本、送批关系与附件");
                ui.label("• 资料库中的全部文档、切块与检索索引");
                ui.label("• 维基库中的全部框架、页面、版本与链接");
                ui.label("• 稿件插图与图表缓存；当前打开的稿件和 PDF 标签将关闭");
                ui.add_space(8.0);
                ui.weak("应用设置、公文词库、校对词表、输入法词表和任务助手数据不会删除。建议先导出全量备份。");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::warning_icon_button(
                            theme::Icon::Trash,
                            "确认永久清空",
                        ))
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            },
        );
        if response.dismissed {
            cancel = true;
        }
        if confirm {
            self.clear_all_content();
        } else if cancel {
            self.content_clear_confirm = false;
        }
    }

    /// 清空四类内容并立即让工作区回到空白状态，避免旧标签在退出时重新入库。
    fn clear_all_content(&mut self) {
        if self.any_busy() {
            self.status = "有其他任务正在运行，请完成后再清空数据。".into();
            return;
        }
        if self.manuscript_store.is_none() {
            self.status = "稿件库不可用，未清空任何内容。".into();
            return;
        }

        // 词库在 config.json，其余三类在 SQLite。先落词库；数据库失败时回滚配置，
        // 避免界面显示已清空而下次启动又恢复旧词库。
        let previous_vocabulary = std::mem::take(&mut self.config.vocabulary);
        if let Err(error) = storage::save(&self.config) {
            self.config.vocabulary = previous_vocabulary;
            self.status = format!("清空失败：标准词库配置保存失败：{error:#}");
            return;
        }
        let result = self
            .manuscript_store
            .as_mut()
            .expect("上面已检查稿件库")
            .clear_all_content();
        if let Err(error) = result {
            self.config.vocabulary = previous_vocabulary;
            let rollback = storage::save(&self.config);
            self.status = match rollback {
                Ok(()) => format!("清空失败，标准词库已恢复：{error:#}"),
                Err(rollback_error) => {
                    format!("清空失败：{error:#}；标准词库配置恢复也失败：{rollback_error:#}")
                }
            };
            return;
        }

        self.vocabulary_selected = None;
        self.vocabulary_collapsed.clear();
        self.vocabulary_delete_confirm = None;
        self.vocabulary_clear_confirm = false;
        self.vocabulary_import_conflicts = None;
        self.vocabulary_sort_undo = None;
        self.vocabulary_move = None;
        self.vocabulary_filter.clear();
        self.vocabulary_dirty = false;

        self.manuscript_rows.clear();
        self.manuscript_selected.clear();
        self.manuscript_count = [0; 4];
        self.manuscript_dirty = false;
        self.manuscript_applied = Some(self.manuscript_filter.clone());
        self.manuscript_delete_confirm = None;
        self.manuscript_delete_refs.clear();
        self.manuscript_batch_delete = None;
        self.manuscript_detail = None;
        self.manuscript_archive_pending = None;
        self.manuscript_versions.clear();
        self.manuscript_package_titles.clear();
        self.manuscript_referrers.clear();
        self.version_commit = None;
        self.version_diff = None;
        self.version_switch = None;
        self.switch_after_commit = None;
        self.revert_confirm = None;
        self.send_package = None;

        self.docs = vec![crate::draft_page::DraftSession::blank(0, &self.config)];
        self.active_doc = 0;
        self.pdfs.clear();
        self.tabs
            .retain(|tab| matches!(tab, super::TabRef::Page(_)));
        if !self
            .tabs
            .contains(&super::TabRef::Page(super::NavPage::Settings))
        {
            self.tabs
                .push(super::TabRef::Page(super::NavPage::Settings));
        }
        self.active_tab = self
            .tabs
            .iter()
            .position(|tab| *tab == super::TabRef::Page(super::NavPage::Settings))
            .unwrap_or(0);

        #[cfg(feature = "ai-workbench")]
        {
            self.knowledge_docs.clear();
            self.knowledge_chunk_count = 0;
            self.knowledge_indexed_manuscripts.clear();
            self.knowledge_embed_models.clear();
            self.knowledge_unindexed = 0;
            self.knowledge_delete_confirm = None;
            self.knowledge_list = Default::default();
            self.knowledge_retrieval_elapsed = None;
            self.knowledge_index_result = None;
            self.knowledge_preview = None;
            self.knowledge_test_results.clear();
            self.knowledge_local_results.clear();
            self.knowledge_search_warnings.clear();
            self.knowledge_qa_history.clear();
            self.knowledge_qa_pending = None;
            self.wiki = Default::default();
            self.wiki_doc_status.clear();
            self.wiki_seen_doc_ids = None;
            self.wiki_new_doc_tip = None;
            self.wiki_circle = None;
            self.refresh_knowledge();
        }
        #[cfg(all(feature = "task-assistant", feature = "ai-workbench"))]
        self.task_ui.invalidate_knowledge_cache();

        self.content_clear_confirm = false;
        self.status = "已清空标准词库、全部文稿、资料库与维基库。".into();
    }

    /// 备份与恢复不能和其他写盘任务并行；有任务在跑时说明原因并拒绝开始。
    fn backup_ready(&mut self) -> bool {
        if self.any_busy() {
            self.status = "有其他任务正在运行，请完成后再导出或恢复备份。".into();
            return false;
        }
        true
    }

    /// 显式导出备份时保存全部已改动稿件，不受定时自动保存开关影响。
    /// 任一保存失败都中止打包，且无论成功与否都还原当前稿件下标。
    fn prepare_backup_export(&mut self) -> anyhow::Result<()> {
        let previous = self.active_doc;
        let result = (|| {
            for index in 0..self.docs.len() {
                let doc = &self.docs[index];
                if doc.is_dirty() || (doc.manuscript_id.is_none() && doc.touched()) {
                    self.active_doc = index;
                    self.try_save_to_manuscript_library()?;
                }
            }
            storage::save(&self.config)
        })();
        self.active_doc = previous;
        result
    }

    /// 选目标路径并在后台打包。打包期间状态栏给出进度提示。
    pub(crate) fn start_backup_export(&mut self) {
        if !self.backup_ready() {
            return;
        }
        if !crate::vault::key_handle().is_some_and(|h| h.db_active()) {
            self.status = "请先启用应用锁并完成加密，再导出全量备份；恢复过程也只写密文。".into();
            return;
        }
        self.open_zip_password_dialog(super::manuscript_ui::ZipPasswordPurpose::BackupExport);
    }

    pub(crate) fn perform_backup_export(&mut self, password: zeroize::Zeroizing<String>) {
        if !self.backup_ready() {
            return;
        }
        let file_name = format!(
            "公文助手备份-{}.7z",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
        // 起始目录跟原来一致：输出目录存在就从它开始挑。
        let output_dir = {
            let trimmed = self.config.output_dir.trim();
            let dir = Path::new(trimmed);
            if !trimmed.is_empty() && dir.is_dir() {
                Some(dir.to_path_buf())
            } else {
                None
            }
        };
        let mut spec = FileDialogSpec::save_file()
            .filter("加密 7z 备份", &["7z"])
            .file_name(file_name);
        if let Some(directory) = output_dir {
            spec = spec.directory(directory);
        }
        // 备份包含数据库、配置、图片与风格档案等用户数据，加密工作区里写成普通 ZIP 要先确认风险。
        self.request_output_with_warning(
            "导出加密 7z 全量备份",
            super::output_guard::BACKUP_WARNING,
            move |app| {
                app.request_file(spec, move |app, path| {
                    let Some(path) = path else {
                        return;
                    };
                    let path = path.with_extension("7z");
                    if let Err(error) = app.prepare_backup_export() {
                        app.status = format!("备份前保存失败，未导出备份：{error:#}");
                        return;
                    }
                    app.backup_busy = true;
                    app.status = format!("正在打包备份到 {}…", path.display());
                    let tx = app.sender.clone();
                    thread::spawn(move || {
                        let result = data_backup::export_backup(&path, &password)
                            .map(|()| {
                                std::fs::metadata(&path)
                                    .map(|metadata| metadata.len())
                                    .unwrap_or(0)
                            })
                            .map_err(|error| format!("{error:#}"));
                        let _ = tx.send(WorkerResult::BackupExported { path, result });
                    });
                });
            },
        );
    }

    /// 选 ZIP 并在后台校验；结果回来后弹二次确认框。
    pub(crate) fn start_backup_inspection(&mut self) {
        if !self.backup_ready() {
            return;
        }
        self.request_file(
            FileDialogSpec::pick_file().filter("加密 7z 备份", &["7z"]),
            |app, path| {
                let Some(path) = path else {
                    return;
                };
                app.open_zip_password_dialog(
                    super::manuscript_ui::ZipPasswordPurpose::BackupImport(path),
                );
            },
        );
    }

    pub(crate) fn perform_backup_inspection(
        &mut self,
        path: PathBuf,
        password: zeroize::Zeroizing<String>,
    ) {
        if !self.backup_ready() {
            return;
        }
        self.backup_busy = true;
        self.backup_restore_password = Some(password.clone());
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result =
                data_backup::inspect_backup(&path, &password).map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::BackupInspected { path, result });
        });
    }

    /// 用户确认后真正执行恢复：先释放 SQLite 连接，再在后台替换数据。
    fn begin_backup_restore(&mut self, zip: PathBuf) {
        if !self.backup_ready() {
            return;
        }
        let config_dir = match storage::config_dir() {
            Ok(dir) => dir,
            Err(error) => {
                self.backup_restore_error = Some(format!("无法确定配置目录：{error:#}"));
                return;
            }
        };
        let Some(password) = self.backup_restore_password.take() else {
            self.backup_restore_error = Some("备份密码已清除，请重新选择备份并输入密码。".into());
            return;
        };
        self.backup_busy = true;
        self.backup_restore_running = true;
        self.status = "正在恢复备份，请勿关闭应用…".into();
        // Windows 下不能替换正被占用的 manuscripts.db；留底目录改名也要求
        // 没有进程继续写这些文件。界面恢复期间是独占模态，不会再有读写。
        self.close_local_stores();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = data_backup::restore_backup(&zip, &config_dir, &password)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::BackupRestored { path: zip, result });
        });
    }

    fn backup_restore_confirm_window(&mut self, ctx: &egui::Context) {
        let Some((zip, info)) = self.backup_restore_prompt.clone() else {
            return;
        };
        let backup_location = storage::config_dir()
            .ok()
            .map(|dir| data_backup::restore_backup_location(&dir));
        let mut confirm = false;
        let mut cancel = false;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("backup_restore_confirm"),
            "恢复备份",
            540.0,
            Dismiss::ButtonsOnly,
            |ui| {
                ui.label(
                    egui::RichText::new("恢复会替换当前全部本地数据！")
                        .strong()
                        .color(warn()),
                );
                ui.add_space(4.0);
                ui.label(format!("备份文件：{}", zip.display()));
                ui.label(format!(
                    "内含 {} 个文件，共 {}。",
                    info.file_count,
                    data_backup::format_bytes(info.total_bytes)
                ));
                ui.add_space(4.0);
                for line in info.summary_lines().iter().take(12) {
                    ui.weak(line);
                }
                if info.groups.len() > 12 {
                    ui.weak(format!("……另有 {} 类内容。", info.groups.len() - 12));
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new("恢复前会原样保留当前数据。若当前数据未加密，留底目录也仍是明文；恢复新版备份不会自动加密或擦除旧副本。").color(theme::danger()));
                if let Some(location) = &backup_location {
                    ui.label(format!("当前数据会先备份到：{}", location.display()));
                }
                ui.weak("恢复完成后需要重启应用。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if theme::primary_icon_button(ui, theme::Icon::RotateCcw, "确认恢复").clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            },
        );
        if response.dismissed {
            cancel = true;
        }
        if confirm {
            // 校验之后到点确认之间可能又起了别的任务；不满足条件就保留确认框，
            // 让用户处理完再点一次。
            if self.backup_ready() {
                self.backup_restore_prompt = None;
                self.begin_backup_restore(zip);
            }
        } else if cancel {
            self.backup_restore_prompt = None;
            self.backup_restore_password = None;
        }
    }

    fn backup_restoring_window(&mut self, ctx: &egui::Context) {
        if !self.backup_restore_running {
            return;
        }
        modal::dialog(
            ctx,
            egui::Id::unique("backup_restoring"),
            "正在恢复备份",
            420.0,
            Dismiss::ButtonsOnly,
            |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(18.0));
                    ui.label("正在替换本地数据，请勿关闭应用…");
                });
                ui.add_space(6.0);
                ui.weak("完成后会提示重启。");
            },
        );
    }

    fn backup_restore_done_window(&mut self, ctx: &egui::Context) {
        let Some(report) = self.backup_restore_result.clone() else {
            return;
        };
        let mut exit = false;
        modal::dialog(
            ctx,
            egui::Id::unique("backup_restore_done"),
            "恢复完成",
            480.0,
            Dismiss::ButtonsOnly,
            |ui| {
                ui.label(egui::RichText::new("恢复完成，请重启应用。").strong());
                ui.add_space(4.0);
                ui.label(format!("已恢复 {} 个文件。", report.restored_files));
                if let Some(dir) = &report.backup_dir {
                    ui.label(format!("恢复前的数据已留底到：{}", dir.display()));
                    ui.weak("确认新数据无误后，可以自行删除这个留底目录。");
                }
                ui.add_space(6.0);
                ui.label("为避免当前运行中的数据覆盖恢复结果，请立即退出后重新打开。");
                ui.add_space(8.0);
                if theme::primary_icon_button(ui, theme::Icon::X, "立即退出").clicked() {
                    exit = true;
                }
            },
        );
        if exit {
            self.backup_restore_result = None;
            // 恢复后不能再走「保存会话 / 自动保存」那条退出路径：库已经换掉了。
            self.exit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn backup_restore_error_window(&mut self, ctx: &egui::Context) {
        let Some(error) = self.backup_restore_error.clone() else {
            return;
        };
        let mut close = false;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("backup_restore_error"),
            "备份恢复未完成",
            480.0,
            Dismiss::ButtonsOnly,
            |ui| {
                ui.label(
                    "没有完成这次备份恢复操作。若数据替换已经开始，程序会尝试回退到恢复前的状态。",
                );
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(egui::RichText::new(&error).monospace())
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Wrap),
                );
                ui.add_space(8.0);
                if ui.button("知道了").clicked() {
                    close = true;
                }
            },
        );
        if close || response.dismissed {
            self.backup_restore_error = None;
        }
    }

    /// 恢复前释放本地库连接；失败时由 [`Self::reopen_local_stores`] 重新打开。
    fn close_local_stores(&mut self) {
        self.manuscript_store = None;
        #[cfg(feature = "ai-workbench")]
        {
            self.knowledge_store = None;
            self.wiki_store = None;
        }
        self.lexicon_store = None;
        #[cfg(feature = "task-assistant")]
        {
            self.task_store = None;
        }
    }

    /// 恢复失败后把三个本地库重新打开，避免界面一直停在「库不可用」。
    pub(crate) fn reopen_local_stores(&mut self) {
        let path = match storage::manuscript_db_path() {
            Ok(path) => path,
            Err(error) => {
                let message = format!("稿件库路径获取失败：{error:#}");
                self.manuscript_error = Some(message.clone());
                #[cfg(feature = "ai-workbench")]
                {
                    self.knowledge_error = Some(message.clone());
                }
                self.lexicon_error = Some(message);
                return;
            }
        };
        self.manuscript_store = match ManuscriptStore::open(&path) {
            Ok(store) => {
                self.manuscript_error = None;
                Some(store)
            }
            Err(error) => {
                self.manuscript_error = Some(format!("稿件库打开失败：{error:#}"));
                None
            }
        };
        #[cfg(feature = "ai-workbench")]
        {
            self.knowledge_store = match KnowledgeStore::open(&path) {
                Ok(store) => {
                    #[cfg(feature = "ai-workbench")]
                    {
                        self.knowledge_error = None;
                    }
                    Some(store)
                }
                Err(error) => {
                    #[cfg(feature = "ai-workbench")]
                    {
                        self.knowledge_error = Some(format!("知识库打开失败：{error:#}"));
                    }
                    None
                }
            };
            (self.wiki_store, self.wiki_error) = match crate::wiki::WikiStore::open(&path) {
                Ok(store) => (Some(store), None),
                Err(error) => (None, Some(format!("维基库打开失败：{error:#}"))),
            };
            self.wiki = crate::wiki_ui::WikiUiState::default();
            // 恢复失败后重置会话状态：晚到的作业结果不能写进重置后的审核台。
            self.wiki_frame_rx = None;
            self.wiki_frame_job = None;
            self.wiki_write_rx = None;
            self.wiki_write_job = None;
            self.wiki_doc_status.clear();
            self.wiki_seen_doc_ids = None;
            self.wiki_new_doc_tip = None;
            self.wiki_circle = None;
        }
        self.lexicon_store = match LexiconStore::open(&path) {
            Ok(store) => {
                crate::lexicon::segmenter::install_from_store(&store);
                self.lexicon_error = None;
                Some(store)
            }
            Err(error) => {
                self.lexicon_error = Some(format!("公文词表打开失败：{error:#}"));
                None
            }
        };
        #[cfg(feature = "task-assistant")]
        {
            self.task_store = match crate::task_assistant::Store::open(&path) {
                Ok(mut store) => {
                    self.task_error = store
                        .bind_legacy_owners(&self.config.vocabulary)
                        .err()
                        .map(|e| format!("负责人身份迁移失败：{e:#}"));
                    Some(store)
                }
                Err(error) => {
                    self.task_error = Some(format!("任务助手数据库重开失败：{error:#}"));
                    None
                }
            };
            // 恢复后不能继续展示旧库的候选或保存旧编辑草稿。
            self.task_ui = crate::task_ui::State::default();
        }
        #[cfg(feature = "ai-workbench")]
        if let Some(job) = self.task_fill_job.take() {
            job.stop();
        }
        #[cfg(feature = "ai-workbench")]
        if let Some(job) = self.task_extraction_job.take() {
            job.stop();
        }
        #[cfg(feature = "ai-workbench")]
        {
            self.task_fill_rx = None;
        }
        #[cfg(feature = "ai-workbench")]
        {
            self.task_extraction_rx = None;
        }
        self.manuscript_dirty = true;
        #[cfg(feature = "ai-workbench")]
        {
            self.knowledge_dirty = true;
        }
        self.lexicon_dirty = true;
    }
}

/// 配置损坏提示。`recovered` 在用户点「知道了」后清空，保证每次启动只弹一次。
fn config_recovery_dialog(ctx: &egui::Context, recovered: &mut Option<PathBuf>) {
    let Some(path) = recovered.clone() else {
        return;
    };
    let response = modal::dialog(
        ctx,
        egui::Id::unique("config_recovered"),
        "配置文件已损坏",
        500.0,
        Dismiss::ButtonsOnly,
        |ui| {
            ui.label("配置文件已损坏，本次以默认设置启动。");
            ui.add_space(6.0);
            ui.label("原文件已备份到：");
            ui.add(
                egui::Label::new(egui::RichText::new(path.display().to_string()).monospace())
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(6.0);
            ui.label("请到设置页检查各项配置。");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("知道了").clicked() {
                    ui.close();
                }
            });
        },
    );
    if response.dismissed {
        *recovered = None;
    }
}

/// 崩溃日志提示。返回「打开日志目录」失败时的状态栏文案。
fn crash_report_dialog(ctx: &egui::Context, pending: &mut Option<PathBuf>) -> Option<String> {
    let path = pending.clone()?;
    let decrypted = crate::crash_log::read_report(&path);
    let can_open_directory = crate::vault::key_handle().is_none();
    let mut open_dir = false;
    let response = modal::dialog(
        ctx,
        egui::Id::unique("crash_report"),
        "上次运行异常退出",
        520.0,
        Dismiss::ButtonsOnly,
        |ui| {
            ui.label("上次运行异常退出，已生成崩溃日志。");
            ui.add_space(6.0);
            ui.label("日志文件：");
            ui.add(
                egui::Label::new(egui::RichText::new(path.display().to_string()).monospace())
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(6.0);
            ui.weak(if can_open_directory {
                "以下为本机诊断内容；把日志发给维护者前请先检查其中是否含本机路径。"
            } else {
                "以下内容仅在已解锁的应用内解密显示；日志文件本身保持密文。"
            });
            egui::ScrollArea::vertical()
                .id_salt("crash_report_detail")
                .max_height(320.0)
                .show(ui, |ui| {
                    let text = match &decrypted {
                        Ok(text) => text.clone(),
                        Err(error) => format!("诊断内容读取失败：{error:#}"),
                    };
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).monospace())
                            .selectable(true)
                            .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if can_open_directory && ui.button("打开日志目录").clicked() {
                    open_dir = true;
                }
                if ui.button("知道了").clicked() {
                    ui.close();
                }
            });
        },
    );
    let mut error = None;
    if open_dir {
        let dir = path.parent().unwrap_or(&path);
        if let Err(message) = open_in_os(dir) {
            error = Some(format!("无法打开日志目录 {}：{message}", dir.display()));
        }
    }
    if response.dismissed {
        *pending = None;
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::NavPage;
    use crate::app::manuscript_ui::ZipPasswordPurpose;
    use crate::models::ManuscriptStatus;
    use crate::test_support::harness_ui;
    use egui_kittest::kittest::Queryable;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 所有文件读写都留在临时配置目录，测试结束时清除线程级覆盖。
    fn with_app(test: impl FnOnce(&mut GongwenApp, &Path)) {
        let dir = tempfile::tempdir().unwrap();
        storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        let mut app = crate::app::management_samples::fixture();
        test(&mut app, dir.path());
    }

    #[test]
    fn backup_saves_existing_and_new_documents_even_when_autosave_is_disabled() {
        with_app(|app, dir| {
            app.config.auto_save = false;
            app.docs[0].generated_markdown = "# 原稿\n\n旧内容。".into();
            app.try_save_to_manuscript_library().unwrap();
            let existing_id = app.docs[0].manuscript_id.unwrap();
            app.docs[0].generated_markdown = "# 原稿\n\n最新修改。".into();
            let mut new = crate::draft_page::DraftSession::blank(1, &app.config);
            new.generated_markdown = "# 新稿\n\n尚未入库。".into();
            app.docs.push(new);
            app.active_doc = 1;

            app.prepare_backup_export().unwrap();

            assert_eq!(app.active_doc, 1);
            assert!(!app.config.auto_save);
            assert!(dir.join("config.json").exists());
            let store = app.manuscript_store.as_mut().unwrap();
            assert_eq!(
                store.get(existing_id).unwrap().unwrap().content_markdown,
                "# 原稿\n\n最新修改。"
            );
            let new_id = app.docs[1].manuscript_id.unwrap();
            assert_eq!(
                store.get(new_id).unwrap().unwrap().content_markdown,
                "# 新稿\n\n尚未入库。"
            );
            assert!(app.docs.iter().all(|doc| !doc.is_dirty()));
        });
    }

    #[test]
    fn backup_save_failure_is_returned_and_restores_the_active_document() {
        with_app(|app, dir| {
            app.docs[0].generated_markdown = "# 尚未保存".into();
            app.docs[0].manuscript_id = Some(-1);
            app.docs
                .push(crate::draft_page::DraftSession::blank(1, &app.config));
            app.active_doc = 1;
            assert!(app.prepare_backup_export().is_err());
            assert_eq!(app.active_doc, 1);
            assert!(app.docs[0].is_dirty());
            assert!(!dir.join("config.json").exists());
            assert!(!app.backup_busy);
        });
    }

    #[test]
    fn lexicon_scan_blocks_backup_and_restore_without_closing_stores() {
        with_app(|app, dir| {
            app.lexicon_busy = true;
            assert!(!app.backup_ready());
            app.begin_backup_restore(dir.join("unused.zip"));
            assert!(!app.backup_restore_running);
            assert!(app.manuscript_store.is_some());
            #[cfg(feature = "ai-workbench")]
            assert!(app.knowledge_store.is_some());
            assert!(app.lexicon_store.is_some());
            app.lexicon_busy = false;
            assert!(app.backup_ready());
        });
    }

    #[test]
    fn update_check_wakes_an_idle_context_when_the_worker_finishes() {
        with_app(|app, _| {
            let ctx = egui::Context::default();
            for _ in 0..3 {
                let _ = crate::test_support::run_ui(&ctx, Default::default(), |_| {});
            }
            let (tx, rx) = std::sync::mpsc::channel();
            ctx.set_request_repaint_callback(move |_| {
                let _ = tx.send(());
            });
            // 不支持的协议在本机立即报错，无需网络或测试 HTTP 服务。
            app.config.update_check_url = "unsupported://example.invalid".into();
            app.start_update_check(&ctx);
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("后台完成时必须唤醒空闲界面");
            app.poll_worker(&ctx);
            assert!(matches!(
                app.update_check,
                crate::update_check::UpdateCheckState::Failed(_)
            ));
        });
    }

    #[test]
    fn backup_password_dialog_renders_without_the_manuscript_page() {
        with_app(|app, _| {
            // 用户停在设置页点「导出备份…」/「恢复备份…」：密码框原先画在稿件库页里，
            // 要切到稿件库才看得见。现在它必须是全局浮窗，这里只画全局那一层来验证。
            app.open_page(NavPage::Settings);
            app.open_zip_password_dialog(ZipPasswordPurpose::BackupImport(PathBuf::from(
                "公文助手备份-20260101-000000.7z",
            )));
            let mut harness = harness_ui(|ui| app.zip_password_window(ui.ctx()));
            harness.run();
            harness.get_by_label("输入备份密码");
            harness.get_by_label("取消").click();
            harness.run();
            drop(harness);
            assert!(app.manuscript_zip_password.is_none(), "取消后应关闭密码框");
        });
    }

    #[test]
    fn content_clear_requires_confirmation_and_clears_all_requested_content() {
        with_app(|app, _| {
            app.docs[0].generated_markdown = "# 待归档\n\n正文".into();
            app.try_save_to_manuscript_library().unwrap();
            let id = app.docs[0].manuscript_id.unwrap();
            let store = app.manuscript_store.as_mut().unwrap();
            store.set_status(id, ManuscriptStatus::Published).unwrap();
            store.set_status(id, ManuscriptStatus::Archived).unwrap();
            app.config.vocabulary.clear();
            app.config.vocabulary.push(crate::models::VocabularyEntry {
                canonical: "测试单位".into(),
                ..Default::default()
            });

            app.content_clear_confirm = true;
            let mut harness = harness_ui(|ui| app.content_clear_confirm_window(ui.ctx()));
            harness.run();
            harness.get_by_label("取消").click();
            harness.run();
            drop(harness);
            assert!(!app.content_clear_confirm);
            assert_eq!(
                app.manuscript_store
                    .as_mut()
                    .unwrap()
                    .count_by_status()
                    .unwrap()[3],
                1,
                "取消确认不能删除归档稿件"
            );
            assert_eq!(app.config.vocabulary.len(), 1);

            app.content_clear_confirm = true;
            let mut harness = harness_ui(|ui| app.content_clear_confirm_window(ui.ctx()));
            harness.run();
            harness.get_by_label("确认永久清空").click();
            harness.run();
            drop(harness);

            assert!(!app.content_clear_confirm);
            assert!(app.config.vocabulary.is_empty());
            assert_eq!(
                app.manuscript_store
                    .as_mut()
                    .unwrap()
                    .count_by_status()
                    .unwrap(),
                [0; 4]
            );
            assert!(app.manuscript_rows.is_empty());
            assert!(app.manuscript_selected.is_empty());
            assert_eq!(app.docs.len(), 1);
            assert_eq!(app.docs[0].manuscript_id, None);
            assert!(!app.docs[0].touched());
            assert!(app.pdfs.is_empty());
            assert!(
                app.tabs
                    .iter()
                    .all(|tab| matches!(tab, super::super::TabRef::Page(_)))
            );
            #[cfg(feature = "ai-workbench")]
            {
                assert!(app.knowledge_docs.is_empty());
                assert_eq!(app.knowledge_chunk_count, 0);
                assert!(app.wiki_doc_status.is_empty());
            }
            let saved = storage::load().unwrap().0;
            assert!(saved.vocabulary.is_empty(), "标准词库必须写入配置");
        });
    }

    #[test]
    fn content_clear_is_rejected_while_jobs_are_running() {
        with_app(|app, _| {
            app.config.vocabulary.clear();
            app.config.vocabulary.push(crate::models::VocabularyEntry {
                canonical: "保留单位".into(),
                ..Default::default()
            });
            app.lexicon_busy = true;
            app.content_clear_confirm = true;
            app.clear_all_content();
            assert_eq!(app.config.vocabulary.len(), 1);
            assert!(app.content_clear_confirm, "忙碌时应保留确认框供稍后重试");
            assert!(app.status.contains("有其他任务正在运行"));
        });
    }

    #[test]
    fn config_recovery_dialog_closes_once_and_clears_the_marker() {
        let recovered = Rc::new(RefCell::new(Some(PathBuf::from(
            "C:/配置/config.json.损坏-20260101-000000",
        ))));
        let state = recovered.clone();
        let mut harness = harness_ui(move |ui| {
            config_recovery_dialog(ui.ctx(), &mut state.borrow_mut());
        });
        harness.run();
        assert!(recovered.borrow().is_some(), "未点确认前应保持提示");
        harness.get_by_label("知道了").click();
        harness.run();
        drop(harness);
        assert!(recovered.borrow().is_none(), "点「知道了」后不应再弹");
    }

    #[test]
    fn crash_report_dialog_shows_path_and_clears_on_acknowledge() {
        let pending = Rc::new(RefCell::new(Some(PathBuf::from(
            "C:/配置/logs/crash-20260101-000000.log",
        ))));
        let state = pending.clone();
        let mut harness = harness_ui(move |ui| {
            crash_report_dialog(ui.ctx(), &mut state.borrow_mut());
        });
        harness.run();
        assert!(pending.borrow().is_some());
        harness.get_by_label("知道了").click();
        harness.run();
        drop(harness);
        assert!(pending.borrow().is_none());
    }
}
