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
                let spec = FileDialogSpec::save_file().file_name(file_name);
                // 词表由标准词库与个人词条组成，加密工作区里导出成普通文本要先确认风险。
                self.request_output_with_warning(
                    if filtered {
                        "导出输入法筛选结果词表"
                    } else {
                        "导出输入法生效词表"
                    },
                    super::output_guard::PLAINTEXT_WARNING,
                    move |app| {
                        app.request_file(spec, move |app, path| {
                            if let Some(path) = path {
                                app.ime.export_text_to(&path, filtered);
                            }
                        });
                    },
                );
            }
            PageAction::ExportPersonalSettings => {
                let spec = FileDialogSpec::save_file().file_name("词表输入法-个人设置.json");
                // 个人设置里带着用户个人词条，加密工作区里导出成普通 JSON 要先确认风险。
                self.request_output_with_warning(
                    "导出输入法个人设置",
                    super::output_guard::PLAINTEXT_WARNING,
                    move |app| {
                        app.request_file(spec, move |app, path| {
                            if let Some(path) = path {
                                app.ime.export_personal_to(&path);
                            }
                        });
                    },
                );
            }
            PageAction::ExportFinalWordTable => {
                let spec = FileDialogSpec::save_file().file_name("词表输入法-最终词表.txt");
                // 最终词表含标准词库与全部个人词条，加密工作区里导出成普通文本要先确认风险。
                self.request_output_with_warning(
                    "导出输入法最终词表",
                    super::output_guard::PLAINTEXT_WARNING,
                    move |app| {
                        app.request_file(spec, move |app, path| {
                            if let Some(path) = path {
                                app.ime.export_final_word_table_to(&path);
                            }
                        });
                    },
                );
            }
            PageAction::ExportPersonalBackup => {
                let spec = FileDialogSpec::save_file().file_name("词表输入法-个人备份.json");
                // 个人备份含全部个人词条与设置，加密工作区里导出成普通文件要先确认风险。
                self.request_output_with_warning(
                    "导出输入法个人备份（词表与设置）",
                    super::output_guard::PLAINTEXT_WARNING,
                    move |app| {
                        app.request_file(spec, move |app, path| {
                            if let Some(path) = path {
                                app.ime.export_personal_backup_to(&path);
                            }
                        });
                    },
                );
            }
        }
    }
}
