//! Password-protected archive delivery for user data outside the draft editor.

use super::GongwenApp;
use super::file_dialog::FileDialogSpec;
use super::jobs::WorkerResult;
use crate::export::protection::Protection;
use std::path::Path;

impl GongwenApp {
    /// Keep the original file format inside an archive with encrypted filenames.
    pub(super) fn request_protected_archive(
        &mut self,
        operation: impl Into<String>,
        file_name: impl AsRef<Path>,
        entries: impl FnOnce() -> anyhow::Result<Vec<(String, Vec<u8>)>> + Send + 'static,
    ) {
        let default_name = file_name.as_ref().with_extension("7z");
        self.request_document_output(operation, move |app| {
            let protection = match Protection::current() {
                Ok(Some(protection)) => protection,
                Ok(None) => {
                    app.status = "加密工作区已锁定，已取消导出。".into();
                    return;
                }
                Err(error) => {
                    app.status = format!("无法导出：{error:#}");
                    return;
                }
            };
            let spec = FileDialogSpec::save_file()
                .filter("加密 7z", &["7z"])
                .file_name(default_name.to_string_lossy());
            app.request_file_with_context(spec, move |app, path, ctx| {
                let Some(path) = path else { return };
                let path = path.with_extension("7z");
                let tx = app.sender.clone();
                app.status = "正在生成并加密导出文件…".into();
                std::thread::spawn(move || {
                    let result = entries()
                        .and_then(|entries| protection.persist_archive(&path, entries))
                        .map_err(|error| format!("{error:#}"));
                    let _ = tx.send(WorkerResult::AuxiliaryArchiveExported(result));
                    ctx.request_repaint();
                });
            });
        });
    }

    pub(super) fn auxiliary_requires_protection(&self) -> bool {
        self.vault_locked || crate::vault::key_handle().is_some()
    }
}
