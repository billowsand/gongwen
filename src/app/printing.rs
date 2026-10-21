//! 打印对话框的外壳：打开时在后台查打印机，点「打印」后在后台出纸，结果回状态栏。
//! 对话框本身在 `crate::print::dialog`，它只收集选择、不碰打印接口。

use crate::app::{GongwenApp, WorkerResult};
use crate::print::{
    DialogOutcome, PrintDialog, PrintJob, PrintRequest, PrinterCaps, PrinterInfo, PrinterState,
};
use eframe::egui;

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
            return;
        };
        let outcome = dialog.ui(ctx);
        // 对话框只提出「要查这台」，线程由外壳起：UI 线程不碰打印接口。
        let caps_request = dialog.take_caps_request().map(|name| (dialog.id(), name));
        let path = dialog.path().to_path_buf();
        if let Some((id, name)) = caps_request {
            self.start_caps_query(id, name, ctx);
        }
        let Some(outcome) = outcome else {
            return;
        };
        self.print_dialog = None;
        match outcome {
            DialogOutcome::Cancel => {}
            DialogOutcome::Print(job) => self.start_print(job, ctx),
            DialogOutcome::SystemDialog => self.start_system_dialog_print(path, ctx),
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
    fn start_print(&mut self, job: PrintJob, ctx: &egui::Context) {
        if let Some(printer) = &job.printer
            && *printer != self.config.print.last_printer
        {
            self.config.print.last_printer = printer.clone();
            // 记不住上次的打印机不影响这次打印，失败了也不打扰。
            let _ = crate::storage::save(&self.config);
        }
        self.status = format!("正在打印 {}…", job.path.display());
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = crate::print::print(&job);
            let _ = tx.send(WorkerResult::PdfPrinted {
                path: job.path,
                printer: job.printer,
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
                result,
            });
            ctx.request_repaint();
        });
    }

    /// 只有 Windows 有「使用系统对话框…」按钮；其他平台不会走到这里。
    #[cfg(not(windows))]
    fn start_system_dialog_print(&mut self, _path: std::path::PathBuf, _ctx: &egui::Context) {}
}
