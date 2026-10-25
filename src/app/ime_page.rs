//! 输入法词表页交给应用层的事：同步词表，以及需要系统文件对话框的导入导出。
//!
//! 文件对话框由应用层在后台线程开（见 [`super::file_dialog`]），主线程每帧
//! 问一次结果；用户挑完再把路径交给输入法。

use super::GongwenApp;
use super::file_dialog::FileDialogSpec;

impl GongwenApp {
    /// 处理输入法词表页登记的动作。
    pub(crate) fn apply_ime_page_action(&mut self, action: crate::ime::PageAction) {
        use crate::ime::PageAction;
        match action {
            PageAction::SyncLexicon => self.sync_lexicon_to_ime(true),
            PageAction::ImportWordTable { four } => {
                self.request_file(
                    FileDialogSpec::pick_file().filter("词表", &["txt", "tsv"]),
                    move |app, path| {
                        if let Some(path) = path {
                            app.ime.stage_import_from(&path, four);
                        }
                    },
                );
            }
            PageAction::RestoreWordTableBackup => {
                self.request_file(
                    FileDialogSpec::pick_file().filter("词表备份", &["json"]),
                    |app, path| {
                        if let Some(path) = path {
                            app.ime.stage_backup_from(&path);
                        }
                    },
                );
            }
            PageAction::ExportWordTable { filtered } => {
                let file_name = if filtered {
                    "词表输入法-筛选结果.txt"
                } else {
                    "词表输入法-生效词表.txt"
                };
                self.request_file(
                    FileDialogSpec::save_file().file_name(file_name),
                    move |app, path| {
                        if let Some(path) = path {
                            app.ime.export_text_to(&path, filtered);
                        }
                    },
                );
            }
            PageAction::ExportPersonalSettings => {
                self.request_file(
                    FileDialogSpec::save_file().file_name("词表输入法-个人设置.json"),
                    |app, path| {
                        if let Some(path) = path {
                            app.ime.export_personal_to(&path);
                        }
                    },
                );
            }
            PageAction::ExportFinalWordTable => {
                self.request_file(
                    FileDialogSpec::save_file().file_name("词表输入法-最终词表.txt"),
                    |app, path| {
                        if let Some(path) = path {
                            app.ime.export_final_word_table_to(&path);
                        }
                    },
                );
            }
            PageAction::ExportPersonalBackup => {
                self.request_file(
                    FileDialogSpec::save_file().file_name("词表输入法-个人备份.json"),
                    |app, path| {
                        if let Some(path) = path {
                            app.ime.export_personal_backup_to(&path);
                        }
                    },
                );
            }
        }
    }
}
