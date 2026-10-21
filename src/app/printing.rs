//! 打印对话框的外壳：打开时在后台查打印机，点「打印」后在后台出纸，结果回状态栏。
//! 对话框本身在 `crate::print::dialog`，它只收集选择、不碰打印接口。

use crate::app::{GongwenApp, WorkerResult};
use crate::models::PrinterPrefs;
use crate::print::{
    DialogOutcome, PrintDialog, PrintJob, PrintOutcome, PrintRequest, PrinterCaps, PrinterInfo,
    PrinterState,
};
use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

impl GongwenApp {
    /// 打开打印对话框，同时在后台列打印机、逐台查状态。已开着的对话框换成新的。
    pub(crate) fn open_print_dialog(&mut self, request: PrintRequest, ctx: &egui::Context) {
        let id = self.next_print_dialog;
        self.next_print_dialog += 1;
        let path = request.path.clone();
        self.print_dialog = Some(PrintDialog::new(
            id,
            request,
            &self.config.print.last_printer,
            self.config.print.printers.clone(),
        ));
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = crate::print::list_printers();
            let names: Vec<String> = result
                .as_ref()
                .map(|printers| {
                    printers
                        .iter()
                        .map(|printer| printer.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let _ = tx.send(WorkerResult::PrintersListed { dialog: id, result });
            ctx.request_repaint();
            // 整份横排、红色版头只能从文件本身测；打不开就是 None，不拦打印。
            let facts = crate::print::inspect_pdf(&path);
            let _ = tx.send(WorkerResult::PdfInspected { dialog: id, facts });
            ctx.request_repaint();
            // 清单先到、状态逐台补：网络打印机查一次可能要几秒，不能让整张清单等它。
            for name in names {
                let state = crate::print::printer_state(&name);
                if tx
                    .send(WorkerResult::PrinterStateChecked {
                        dialog: id,
                        name,
                        state,
                    })
                    .is_err()
                {
                    return;
                }
                ctx.request_repaint();
            }
        });
    }

    pub(crate) fn apply_printers_listed(
        &mut self,
        dialog: u64,
        result: Result<Vec<PrinterInfo>, String>,
    ) {
        if let Some(open) = self
            .print_dialog
            .as_mut()
            .filter(|open| open.id() == dialog)
        {
            open.printers_loaded(result);
        }
    }

    pub(crate) fn apply_printer_state(&mut self, dialog: u64, name: &str, state: PrinterState) {
        if let Some(open) = self
            .print_dialog
            .as_mut()
            .filter(|open| open.id() == dialog)
        {
            open.printer_state(name, state);
        }
    }

    /// 弹驱动自己的首选项窗口：会阻塞等用户操作，只能在后台线程。
    /// 只有 Windows 真去弹；非 Windows 上对话框不画这个按钮。
    #[cfg(windows)]
    fn start_preferences_edit(
        &mut self,
        dialog: u64,
        printer: String,
        current: Option<Vec<u8>>,
        ctx: &egui::Context,
    ) {
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = crate::print::edit_preferences(&printer, current.as_deref());
            let _ = tx.send(WorkerResult::PreferencesEdited {
                dialog,
                printer,
                result,
            });
            ctx.request_repaint();
        });
    }

    pub(crate) fn apply_preferences_edited(
        &mut self,
        dialog: u64,
        printer: &str,
        result: Result<Option<Vec<u8>>, String>,
    ) {
        if let Some(open) = self
            .print_dialog
            .as_mut()
            .filter(|open| open.id() == dialog)
        {
            open.preferences_edited(printer, result);
        }
    }

    pub(crate) fn apply_pdf_inspected(
        &mut self,
        dialog: u64,
        facts: Option<crate::print::PdfFacts>,
    ) {
        if let Some(open) = self
            .print_dialog
            .as_mut()
            .filter(|open| open.id() == dialog)
        {
            open.facts_loaded(facts);
        }
    }

    pub(crate) fn apply_printer_caps(&mut self, dialog: u64, name: &str, caps: PrinterCaps) {
        if let Some(open) = self
            .print_dialog
            .as_mut()
            .filter(|open| open.id() == dialog)
        {
            open.caps_loaded(name, caps);
        }
    }

    pub(crate) fn print_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.print_dialog.as_mut() else {
            // 对话框关了：不再钉住预览页，PDF 标签的纹理淘汰照常回收。
            for pdf in &mut self.pdfs {
                pdf.stop_preview();
            }
            return;
        };
        let dialog_id = dialog.id();
        // 预览从 PDF 标签已经渲染好的纹理来，不为预览另起渲染线程。PDF 标签不在
        // 前台时它的 `ui` 不跑，这里把对话框想看的那一页塞进它已有的渲染队列。
        let pdf_key = dialog.pdf_key();
        if let Some(pdf) = pdf_key.and_then(|key| self.pdfs.iter_mut().find(|pdf| pdf.key == key)) {
            match dialog.wanted_preview() {
                Some(index) => pdf.want_preview(index),
                None => pdf.stop_preview(),
            }
            pdf.pump_preview(ctx);
        }
        let outcome = {
            let pdfs = &mut self.pdfs;
            let mut textures = |index: usize| -> Option<crate::print::PreviewPage> {
                let key = pdf_key?;
                pdfs.iter_mut()
                    .find(|pdf| pdf.key == key)?
                    .preview_page(index)
            };
            dialog.ui(ctx, &mut textures)
        };
        // 对话框只提出「要查这台」「要弹首选项」，线程由外壳起：UI 线程不碰打印接口。
        let caps_request = dialog.take_caps_request().map(|name| (dialog_id, name));
        let preferences_request = dialog
            .take_preferences_request()
            .map(|(name, devmode)| (dialog_id, name, devmode));
        let path = dialog.path().to_path_buf();
        // 点「打印」时的选择要写回配置（和「上次用的打印机」同处保存）。
        let printer_prefs = dialog.preferences();
        // 「反面倒序」这类改了即记住的偏好，当帧就落盘。
        let prefs_update = dialog.take_preferences_update();
        if let Some((id, name)) = caps_request {
            self.start_caps_query(id, name, ctx);
        }
        #[cfg(windows)]
        if let Some((id, name, devmode)) = preferences_request {
            self.start_preferences_edit(id, name, devmode, ctx);
        }
        // 非 Windows 上对话框不画「打印机首选项…」按钮（`HAS_DRIVER_PREFERENCES`），
        // 这里收不到请求。万一收到，当作用户取消走同一条结果通道：对话框把挂起标志
        // 复位，不弹窗口，也不改动任何设置。
        #[cfg(not(windows))]
        if let Some((dialog, printer, _devmode)) = preferences_request {
            let _ = self.sender.send(WorkerResult::PreferencesEdited {
                dialog,
                printer,
                result: Ok(None),
            });
        }
        if let Some((printer, prefs)) = prefs_update {
            self.config.print.printers.insert(printer, prefs);
            let _ = crate::storage::save(&self.config);
        }
        let Some(outcome) = outcome else {
            return;
        };
        match outcome {
            DialogOutcome::Cancel => self.print_dialog = None,
            // 打印中对话框不关：显示进度与「取消」，结果按 dialog 编号转回来
            // （手动双面第一轮还会停在「等待翻面」）。
            DialogOutcome::Print(job) => {
                let cancel = self
                    .print_dialog
                    .as_ref()
                    .map(PrintDialog::cancel_token)
                    .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
                self.start_print(job, printer_prefs, Some(dialog_id), cancel, ctx);
            }
            DialogOutcome::SystemDialog => {
                self.print_dialog = None;
                self.start_system_dialog_print(path, ctx);
            }
        }
    }

    /// 打印任务结束：状态栏照旧，手动双面的一轮还要把结果转回对话框。
    pub(crate) fn apply_pdf_printed(
        &mut self,
        path: std::path::PathBuf,
        printer: Option<String>,
        dialog: Option<u64>,
        result: Result<PrintOutcome, String>,
    ) {
        self.status = match &result {
            Ok(PrintOutcome::Printed { job }) => match (printer, job) {
                (Some(printer), Some(job)) => format!(
                    "已送打印机「{printer}」（任务 {job}）：{}。",
                    path.display()
                ),
                (Some(printer), None) => format!("已送打印机「{printer}」：{}。", path.display()),
                (None, Some(job)) => {
                    format!("已送打印机（任务 {job}）：{}。", path.display())
                }
                (None, None) => format!("已送打印机：{}。", path.display()),
            },
            Ok(PrintOutcome::Cancelled) => "已取消打印。".into(),
            Err(error) => format!("打印失败：{error}"),
        };
        let Some(id) = dialog else {
            return;
        };
        let close = self
            .print_dialog
            .as_mut()
            .filter(|open| open.id() == id)
            .is_some_and(|open| open.manual_finished(&result));
        if close {
            self.print_dialog = None;
        }
    }

    /// 查一台打印机支持的双面、颜色、纸张与纸盒。查一次可能几百毫秒
    /// （网络打印机更久），放后台线程，对话框先画「正在读取打印机能力…」。
    fn start_caps_query(&mut self, dialog: u64, name: String, ctx: &egui::Context) {
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let caps = crate::print::printer_caps(&name);
            let _ = tx.send(WorkerResult::PrinterCapsChecked { dialog, name, caps });
            ctx.request_repaint();
        });
    }

    /// 出纸可能要几秒（逐页光栅化或等 `lp`），放后台线程，主界面照常可用。
    /// 后台报来的打印进度。对话框已经关了就丢掉。
    pub(crate) fn apply_print_progress(&mut self, dialog: u64, done: u32, total: u32) {
        if let Some(open) = self
            .print_dialog
            .as_mut()
            .filter(|open| open.id() == dialog)
        {
            open.progress(done, total);
        }
    }

    fn start_print(
        &mut self,
        job: PrintJob,
        prefs: Option<(String, PrinterPrefs)>,
        dialog: Option<u64>,
        cancel: Arc<AtomicBool>,
        ctx: &egui::Context,
    ) {
        let mut changed = false;
        if let Some(printer) = &job.printer
            && *printer != self.config.print.last_printer
        {
            self.config.print.last_printer = printer.clone();
            changed = true;
        }
        if let Some((printer, prefs)) = prefs {
            self.config.print.printers.insert(printer, prefs);
            changed = true;
        }
        if changed {
            // 记不住偏好不影响这次打印，失败了也不打扰。
            let _ = crate::storage::save(&self.config);
        }
        self.status = format!("正在打印 {}…", job.path.display());
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        let progress_tx = self.sender.clone();
        let progress_ctx = ctx.clone();
        std::thread::spawn(move || {
            let progress = move |done: u32, total: u32| {
                let _ = progress_tx.send(WorkerResult::PrintProgress {
                    dialog: dialog.unwrap_or(0),
                    done,
                    total,
                });
                progress_ctx.request_repaint();
            };
            let result = crate::print::print(&job, &progress, &cancel);
            let _ = tx.send(WorkerResult::PdfPrinted {
                path: job.path,
                printer: job.printer,
                dialog,
                result,
            });
            // 主线程可能正闲着睡着，主动敲醒它。
            ctx.request_repaint();
        });
    }

    #[cfg(windows)]
    fn start_system_dialog_print(&mut self, path: std::path::PathBuf, ctx: &egui::Context) {
        self.status = format!("正在准备打印 {}…", path.display());
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = crate::print::print_with_system_dialog(&path);
            let _ = tx.send(WorkerResult::PdfPrinted {
                path,
                printer: None,
                dialog: None,
                result,
            });
            ctx.request_repaint();
        });
    }

    /// 只有 Windows 有「使用系统对话框…」按钮；其他平台不会走到这里。
    #[cfg(not(windows))]
    fn start_system_dialog_print(&mut self, _path: std::path::PathBuf, _ctx: &egui::Context) {}
}
