//! 文件汇编：把勾选的稿件按列表顺序编译、合并成一份内存 PDF，打开后可打印。
//!
//! 先不做目录与连续页码；这两项等 `RenderPlan`/`BatchPlan` 接入后再加。

use crate::app::GongwenApp;
use crate::app::jobs::WorkerResult;
use crate::manuscript::ManuscriptStore;
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use anyhow::{Context, Result, bail};
use eframe::egui;

impl GongwenApp {
    pub(crate) fn start_file_compilation(&mut self, ctx: &egui::Context) {
        let ids = self
            .manuscript_rows
            .iter()
            .filter(|row| self.manuscript_selected.contains(&row.id))
            .map(|row| row.id)
            .collect::<Vec<_>>();
        if ids.len() < 2 {
            self.status = "至少勾选两篇稿件再合并。".into();
            return;
        }
        let db_path = match crate::storage::manuscript_db_path() {
            Ok(path) => path,
            Err(error) => {
                self.status = format!("无法定位稿件库：{error:#}");
                return;
            }
        };
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
        let numbering = self.config.numbering;
        let tx = self.sender.clone();
        let wake = ctx.clone();
        self.status = format!("正在编译并合并 {} 篇稿件…", ids.len());
        std::thread::spawn(move || {
            let result = build_compilation(&db_path, &ids, &vocabulary, &fonts, numbering)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::CompilationMerged(result));
            wake.request_repaint();
        });
    }
}

fn build_compilation(
    db_path: &std::path::Path,
    ids: &[i64],
    vocabulary: &[crate::models::VocabularyEntry],
    fonts: &crate::models::FontConfig,
    numbering: crate::models::NumberingConfig,
) -> Result<Vec<u8>> {
    if ids.len() < 2 {
        bail!("至少需要两篇稿件");
    }
    let store = ManuscriptStore::open(db_path)?;
    let display = UnitDisplay::new(vocabulary);
    let (fonts, _warnings) = crate::system_fonts::resolve(fonts);
    let mut parts = Vec::with_capacity(ids.len());
    for id in ids {
        let record = store
            .get_for_editor(*id)?
            .with_context(|| format!("稿件 {id} 不存在"))?;
        let outcome = crate::export::compile_pdf(
            &record.snapshot,
            &record.content_markdown,
            &display,
            &fonts,
            &numbering,
            &ElementMarks::default(),
        )
        .with_context(|| format!("《{}》编译失败", record.title))?;
        parts.push(outcome.pdf);
    }
    Ok(crate::manuscript_io::send_package::merge_pdfs(&parts)?.bytes)
}
