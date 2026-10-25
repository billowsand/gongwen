//! 数据安全相关的界面：启动时的配置 / 崩溃日志提示，以及设置页的一键备份与恢复。
//!
//! 三个提示都走 [`crate::modal`] 的独占模态框：启动提示只弹一次，备份恢复在
//! 执行期间挡住其他操作，避免用户一边恢复一边编辑。
//! 备份文件的读写与回滚在 [`crate::data_backup`]，这里只负责交互与状态。

use super::file_dialog::FileDialogSpec;
use super::settings::{setting_continuation, setting_row, sub_heading};
use super::{GongwenApp, WorkerResult, open_in_os, warn};
use crate::data_backup;
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
            Some("把配置目录里的有效数据打成一个 ZIP，换机器或重装系统前先导出一份。"),
        );
        ui.label(
            "备份包含配置、稿件库与知识库（manuscripts.db）、图片、数据接口配置、\
             输入法词表与技能文件。崩溃日志、崩溃提示标记与损坏配置的留底文件属于\
             本机诊断信息，不进入备份。",
        );
        ui.add_space(8.0);

        let mut export = false;
        let mut inspect = false;
        let idle = !self.any_busy();
        ui.add_enabled_ui(idle, |ui| {
            setting_row(ui, "导出备份", None, |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Archive, "导出备份…").clicked()
                {
                    export = true;
                }
                ui.weak("选一个配置目录之外的位置保存 ZIP。");
            });
            setting_continuation(ui, |ui| {
                ui.weak("导出前会保存所有已修改稿件；保存失败将取消备份。");
                ui.weak("导出期间请勿关闭应用；完成后状态栏会显示文件大小。");
            });
            ui.add_space(8.0);
            setting_row(ui, "恢复备份", None, |ui| {
                if ui
                    .add(theme::secondary_icon_button(
                        theme::Icon::RotateCcw,
                        "恢复备份…",
                    ))
                    .clicked()
                {
                    inspect = true;
                }
                ui.weak("先校验文件，再让你确认一次才会动数据。");
            });
            if let Ok(dir) = storage::config_dir() {
                let location = data_backup::restore_backup_location(&dir);
                setting_continuation(ui, |ui| {
                    ui.weak(format!(
                        "恢复前会把当前数据整体留底到同级目录：{}",
                        location.display()
                    ));
                });
            }
        });
        if self.backup_busy {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.weak("正在处理备份…");
            });
        } else if !idle {
            ui.add_space(6.0);
            ui.weak("有其他任务正在运行，请完成后再导出或恢复备份。");
        }
        if export {
            self.start_backup_export();
        }
        if inspect {
            self.start_backup_inspection();
        }
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
        let file_name = format!(
            "公文助手备份-{}.zip",
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
            .filter("ZIP 备份", &["zip"])
            .file_name(file_name);
        if let Some(directory) = output_dir {
            spec = spec.directory(directory);
        }
        self.request_file(spec, move |app, path| {
            let Some(path) = path else {
                return;
            };
            if let Err(error) = app.prepare_backup_export() {
                app.status = format!("备份前保存失败，未导出备份：{error:#}");
                return;
            }
            app.backup_busy = true;
            app.status = format!("正在打包备份到 {}…", path.display());
            let tx = app.sender.clone();
            thread::spawn(move || {
                let result = data_backup::export_backup(&path)
                    .map(|()| {
                        std::fs::metadata(&path)
                            .map(|metadata| metadata.len())
                            .unwrap_or(0)
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = tx.send(WorkerResult::BackupExported { path, result });
            });
        });
    }

    /// 选 ZIP 并在后台校验；结果回来后弹二次确认框。
    pub(crate) fn start_backup_inspection(&mut self) {
        if !self.backup_ready() {
            return;
        }
        self.request_file(
            FileDialogSpec::pick_file().filter("ZIP 备份", &["zip"]),
            |app, path| {
                let Some(path) = path else {
                    return;
                };
                app.backup_busy = true;
                app.status = format!("正在校验备份 {}…", path.display());
                let tx = app.sender.clone();
                thread::spawn(move || {
                    let result =
                        data_backup::inspect_backup(&path).map_err(|error| format!("{error:#}"));
                    let _ = tx.send(WorkerResult::BackupInspected { path, result });
                });
            },
        );
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
        self.backup_busy = true;
        self.backup_restore_running = true;
        self.status = "正在恢复备份，请勿关闭应用…".into();
        // Windows 下不能替换正被占用的 manuscripts.db；留底目录改名也要求
        // 没有进程继续写这些文件。界面恢复期间是独占模态，不会再有读写。
        self.close_local_stores();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = data_backup::restore_backup(&zip, &config_dir)
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
            egui::Id::new("backup_restore_confirm"),
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
        }
    }

    fn backup_restoring_window(&mut self, ctx: &egui::Context) {
        if !self.backup_restore_running {
            return;
        }
        modal::dialog(
            ctx,
            egui::Id::new("backup_restoring"),
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
            egui::Id::new("backup_restore_done"),
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
            egui::Id::new("backup_restore_error"),
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
        self.knowledge_store = None;
        self.lexicon_store = None;
    }

    /// 恢复失败后把三个本地库重新打开，避免界面一直停在「库不可用」。
    pub(crate) fn reopen_local_stores(&mut self) {
        let path = match storage::manuscript_db_path() {
            Ok(path) => path,
            Err(error) => {
                let message = format!("稿件库路径获取失败：{error:#}");
                self.manuscript_error = Some(message.clone());
                self.knowledge_error = Some(message.clone());
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
        self.knowledge_store = match KnowledgeStore::open(&path) {
            Ok(store) => {
                self.knowledge_error = None;
                Some(store)
            }
            Err(error) => {
                self.knowledge_error = Some(format!("知识库打开失败：{error:#}"));
                None
            }
        };
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
        self.manuscript_dirty = true;
        self.knowledge_dirty = true;
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
        egui::Id::new("config_recovered"),
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
    let mut open_dir = false;
    let response = modal::dialog(
        ctx,
        egui::Id::new("crash_report"),
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
            ui.weak("把这份日志发给维护者，可以定位闪退原因。");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("打开日志目录").clicked() {
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
    use egui_kittest::Harness;
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
    fn config_recovery_dialog_closes_once_and_clears_the_marker() {
        let recovered = Rc::new(RefCell::new(Some(PathBuf::from(
            "C:/配置/config.json.损坏-20260101-000000",
        ))));
        let state = recovered.clone();
        let mut harness = Harness::new_ui(move |ui| {
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
        let mut harness = Harness::new_ui(move |ui| {
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
