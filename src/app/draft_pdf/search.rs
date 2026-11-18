//! 从内存 PDF 的文字层查找命中页。放在后台扫描，翻页不等检索。

use crate::app::WorkerResult;
use crate::pdf_viewer::PdfSession;
use crate::{pdf_text, theme};
use eframe::egui;
use hayro::hayro_interpret::{InterpreterCache, hayro_syntax::Pdf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
};

const FIND_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::F);

#[derive(Default)]
pub(in crate::app) struct Search {
    open: bool,
    focus_query: bool,
    query: String,
    scan: u64,
    cancel: Option<Arc<AtomicBool>>,
    result: Option<Result<Vec<usize>, String>>,
    selected: usize,
    jump: bool,
}

impl Search {
    pub(super) fn button(&mut self, ui: &mut egui::Ui, pdf: &PdfSession) {
        let ready = pdf.page_size_pt(0).is_some();
        let clicked = theme::icon_button_enabled(ui, ready, theme::Icon::Search, "在 PDF 中查找")
            .on_hover_text(format!(
                "在 PDF 中查找（{}）",
                ui.ctx().format_shortcut(&FIND_SHORTCUT)
            ))
            .clicked();
        let shortcut = ready
            && crate::modal::takes_keys(ui)
            && ui.input_mut(|input| input.consume_shortcut(&FIND_SHORTCUT));
        if clicked || shortcut {
            self.open = true;
            self.focus_query = true;
        }
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        pdf: &mut PdfSession,
        results: &Sender<WorkerResult>,
    ) {
        if !self.open {
            return;
        }
        let mut submit = false;
        let mut close = false;
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("查找文字")
                    .desired_width(180.0)
                    .char_limit(200),
            );
            if std::mem::take(&mut self.focus_query) {
                response.request_focus();
            }
            if response.changed() {
                self.clear_results();
            }
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            submit = ui
                .add_enabled(!self.query.trim().is_empty(), egui::Button::new("查找"))
                .clicked()
                || enter;
            close = theme::icon_button(ui, theme::Icon::X, "关闭查找").clicked()
                || (response.has_focus()
                    && ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                    }));
            self.navigation(ui);
        });
        if close {
            self.open = false;
            self.clear_results();
        } else if submit {
            self.start(pdf, ui.ctx(), results);
        }
        if std::mem::take(&mut self.jump)
            && let Some(Ok(pages)) = &self.result
            && let Some(&page) = pages.get(self.selected)
        {
            pdf.go_to_page(page);
        }
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        match &self.result {
            Some(Ok(pages)) if !pages.is_empty() => {
                let total = pages.len();
                if theme::icon_button_enabled(
                    ui,
                    self.selected > 0,
                    theme::Icon::ChevronUp,
                    "上一处命中页",
                )
                .clicked()
                {
                    self.selected -= 1;
                    self.jump = true;
                }
                if theme::icon_button_enabled(
                    ui,
                    self.selected + 1 < total,
                    theme::Icon::ChevronDown,
                    "下一处命中页",
                )
                .clicked()
                {
                    self.selected += 1;
                    self.jump = true;
                }
                ui.label(format!("命中页 {} / {total}", self.selected + 1));
            }
            Some(Ok(_)) => {
                ui.label("未找到匹配文字");
            }
            Some(Err(error)) => {
                ui.colored_label(theme::danger(), error);
            }
            None if self.cancel.is_some() => {
                ui.spinner();
                ui.label("正在查找…");
            }
            None => {}
        }
    }

    fn start(&mut self, pdf: &PdfSession, ctx: &egui::Context, results: &Sender<WorkerResult>) {
        let query = normalize(&self.query);
        let Some(bytes) = pdf.memory_bytes().filter(|_| !query.is_empty()) else {
            return;
        };
        self.clear_results();
        self.scan += 1;
        let scan = self.scan;
        let key = pdf.key;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(Arc::clone(&cancel));
        let tx = results.clone();
        let ctx = ctx.clone();
        if let Err(error) = std::thread::Builder::new().spawn(move || {
            let result = matching_pages(bytes, &query, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(WorkerResult::DraftPdfSearch { key, scan, result });
                ctx.request_repaint();
            }
        }) {
            self.receive(scan, Err(format!("无法启动查找：{error}")));
        }
    }

    pub(in crate::app) fn receive(&mut self, scan: u64, result: Result<Vec<usize>, String>) {
        if self.scan == scan && self.cancel.take().is_some() {
            self.result = Some(result);
            self.jump = true;
        }
    }

    pub(super) fn clear_results(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.result = None;
        self.selected = 0;
        self.jump = false;
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.clear_results();
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn matching_pages(
    bytes: Arc<Vec<u8>>,
    query: &str,
    cancel: &AtomicBool,
) -> Result<Vec<usize>, String> {
    let pdf = Pdf::new(bytes).map_err(|error| format!("无法读取 PDF 文字：{error:?}"))?;
    let cache = InterpreterCache::new();
    let mut hits = Vec::new();
    for (index, page) in pdf.pages().iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let text = pdf_text::extract(page, &cache);
        if normalize(&text.text(0, text.glyphs.len())).contains(query) {
            hits.push(index);
        }
    }
    Ok(hits)
}
