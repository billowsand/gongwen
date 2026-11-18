//! 起草页的只读 PDF 预览。标签及排版结果随工作区销毁。

mod search;
#[cfg(test)]
mod tests;

use super::{GongwenApp, TabRef, WorkerResult};
use crate::draft_page::{DocKey, DraftSession};
use crate::models::{
    AppConfig, DiagramTheme, DraftInput, FontConfig, NumberingConfig, VocabularyEntry,
};
use crate::pdf_viewer::{PdfKey, PdfSession};
use crate::{export, mermaid, storage, theme, typst_engine::TypstOutcome};
use eframe::egui;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct DraftPdfPreviews {
    pub(super) tabs: HashMap<DocKey, DraftPdfPreview>,
    /// 关闭标签后仍登记未退出的排版任务，锁定时等它结束读库再切换槽位。
    compiling: HashMap<DocKey, PdfKey>,
}

impl DraftPdfPreviews {
    pub(super) fn is_compiling(&self, doc: DocKey) -> bool {
        self.compiling.contains_key(&doc)
    }

    pub(super) fn by_pdf_key(&mut self, key: PdfKey) -> Option<&mut DraftPdfPreview> {
        self.tabs
            .values_mut()
            .find(|preview| preview.pdf.as_ref().is_some_and(|pdf| pdf.key == key))
    }
}

#[derive(Default)]
pub(super) struct DraftPdfPreview {
    pub(super) pdf: Option<PdfSession>,
    pub(super) search: search::Search,
    displayed: Option<Arc<PdfInput>>,
    pending: Option<Arc<PdfInput>>,
    warnings: Vec<String>,
    error: Option<String>,
}

struct PdfInput {
    draft: DraftInput,
    markdown: String,
    vocabulary: Vec<VocabularyEntry>,
    fonts: FontConfig,
    numbering: NumberingConfig,
    diagram_theme: DiagramTheme,
}

impl PdfInput {
    fn capture(doc: &DraftSession, config: &AppConfig) -> Self {
        Self {
            draft: doc.draft.clone(),
            markdown: doc.generated_markdown.clone(),
            vocabulary: config.vocabulary.clone(),
            fonts: config.fonts.clone(),
            numbering: config.numbering,
            diagram_theme: mermaid::current_theme(),
        }
    }

    fn matches(&self, doc: &DraftSession, config: &AppConfig) -> bool {
        self.draft == doc.draft
            && self.markdown == doc.generated_markdown
            && self.fonts == config.fonts
            && self.numbering == config.numbering
            && self.diagram_theme == mermaid::current_theme()
            && self.vocabulary == config.vocabulary
    }

    fn compile(&self, base_dir: &Path) -> anyhow::Result<TypstOutcome> {
        export::compile_pdf_preview(
            &self.draft,
            &self.markdown,
            &self.vocabulary,
            &self.fonts,
            &self.numbering,
            base_dir,
        )
    }
}

impl GongwenApp {
    pub(super) fn start_draft_pdf(&mut self, doc: DocKey, ctx: &egui::Context, refresh: bool) {
        let Some(index) = self.doc_index_of_key(doc) else {
            return;
        };
        if self.lock_requested || storage::writes_blocked() {
            return;
        }
        let source = &self.docs[index];
        let existing = self
            .tabs
            .iter()
            .position(|tab| *tab == TabRef::DraftPdf(doc));
        if self.draft_pdfs.is_compiling(doc) {
            if let Some(tab) = existing {
                self.activate_tab(tab);
            } else {
                self.status = "上一次 PDF 排版正在结束，请稍后重试。".into();
            }
            return;
        }
        if source.busy || source.generated_markdown.trim().is_empty() {
            return;
        }
        if !refresh
            && let Some(tab) = existing
            && let Some(preview) = self.draft_pdfs.tabs.get(&doc)
            && preview.error.is_none()
            && preview
                .displayed
                .as_ref()
                .is_some_and(|input| input.matches(source, &self.config))
        {
            self.activate_tab(tab);
            return;
        }

        let input = Arc::new(PdfInput::capture(source, &self.config));
        let key = self.next_pdf_key;
        self.next_pdf_key += 1;
        let preview = self.draft_pdfs.tabs.entry(doc).or_default();
        preview.pending = Some(Arc::clone(&input));
        preview.error = None;
        self.draft_pdfs.compiling.insert(doc, key);

        let tab = existing.unwrap_or_else(|| {
            self.tabs.push(TabRef::DraftPdf(doc));
            self.tabs.len() - 1
        });
        self.activate_tab(tab);

        let tx = self.sender.clone();
        let repaint = ctx.clone();
        let base_dir = storage::config_dir();
        if let Err(error) = std::thread::Builder::new().spawn(move || {
            let result = std::panic::catch_unwind(|| input.compile(&base_dir?))
                .map_err(|_| "排版器意外中止，请重试。".to_string())
                .and_then(|result| result.map_err(|error| format!("{error:#}")));
            let _ = tx.send(WorkerResult::DraftPdf { doc, key, result });
            repaint.request_repaint();
        }) {
            self.finish_draft_pdf(doc, key, Err(format!("无法启动排版：{error}")));
        }
    }

    pub(super) fn finish_draft_pdf(
        &mut self,
        doc: DocKey,
        key: PdfKey,
        result: Result<TypstOutcome, String>,
    ) {
        if self.draft_pdfs.compiling.get(&doc) != Some(&key) {
            return;
        }
        self.draft_pdfs.compiling.remove(&doc);
        let Some(preview) = self.draft_pdfs.tabs.get_mut(&doc) else {
            return;
        };
        let pending = preview.pending.take();
        match result {
            Ok(outcome) => {
                let mut pdf = PdfSession::from_memory(
                    key,
                    format!("draft-preview-{key}").into(),
                    "PDF 预览".into(),
                    outcome.pdf,
                    self.sender.clone(),
                );
                if let Some(previous) = &preview.pdf {
                    pdf.inherit_view(previous);
                }
                preview.pdf = Some(pdf);
                preview.displayed = pending;
                preview.warnings = outcome.warnings;
                preview.search.clear_results();
            }
            Err(error) => preview.error = Some(error),
        }
    }

    pub(super) fn draft_pdf_is_compiling(&self) -> bool {
        !self.draft_pdfs.compiling.is_empty()
    }

    pub(super) fn discard_draft_pdf_previews(&mut self) {
        if self.draft_pdfs.tabs.is_empty() {
            return;
        }
        self.draft_pdfs.tabs.clear();
        let active = self
            .tabs
            .get(self.active_tab)
            .copied()
            .map(|tab| match tab {
                TabRef::DraftPdf(doc) => TabRef::Doc(doc),
                tab => tab,
            });
        self.tabs.retain(|tab| !matches!(tab, TabRef::DraftPdf(_)));
        self.active_tab = self
            .tabs
            .iter()
            .position(|tab| Some(*tab) == active)
            .unwrap_or(0);
        if let Some(TabRef::Doc(key)) = self.tabs.get(self.active_tab)
            && let Some(index) = self.doc_index_of_key(*key)
        {
            self.active_doc = index;
        }
    }

    pub(super) fn draft_pdf_ui(&mut self, doc: DocKey, ui: &mut egui::Ui) {
        let Some(index) = self.doc_index_of_key(doc) else {
            return;
        };
        let Some(preview) = self.draft_pdfs.tabs.get_mut(&doc) else {
            return;
        };
        let source = &self.docs[index];
        let stale = preview
            .displayed
            .as_ref()
            .is_some_and(|input| !input.matches(source, &self.config));
        let compiling = preview.pending.is_some();
        let can_refresh =
            !compiling && !source.busy && !source.generated_markdown.trim().is_empty();
        let mut back = false;
        let mut refresh = false;
        egui::Panel::top(egui::Id::unique("draft_pdf_actions").with(doc))
            .frame(theme::panel(theme::surface(), 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    back = ui.button("返回稿件").clicked();
                    refresh = ui
                        .add_enabled(
                            can_refresh,
                            theme::icon_text_button(theme::Icon::Refresh, "刷新预览"),
                        )
                        .clicked();
                    if let Some(pdf) = &preview.pdf {
                        preview.search.button(ui, pdf);
                    }
                    if compiling {
                        ui.spinner();
                        ui.label("正在排版 PDF…");
                    } else if stale {
                        ui.colored_label(theme::text_muted(), "稿件已修改，请刷新预览");
                    }
                });
                if let Some(error) = &preview.error {
                    ui.colored_label(theme::danger(), format!("PDF 排版失败：{error}"));
                }
                if !preview.warnings.is_empty() {
                    egui::CollapsingHeader::new("排版提示").show(ui, |ui| {
                        for warning in &preview.warnings {
                            ui.label(warning);
                        }
                    });
                }
                if let Some(pdf) = &mut preview.pdf {
                    preview.search.ui(ui, pdf, &self.sender);
                }
            });
        if let Some(pdf) = &mut preview.pdf {
            pdf.reading_ui(ui);
        }
        if back {
            self.activate_doc(index);
        } else if refresh {
            self.start_draft_pdf(doc, ui.ctx(), true);
        }
    }
}
