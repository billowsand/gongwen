//! 后台任务与导入导出：生成/优化/导出/文件导入。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::tasks`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::{DocJob, DraftAction, WorkerResult, accent, export_and_compile, reveal_in_os};
use crate::doc_import;
use crate::draft_page::{DocKey, DraftPage, ExportKind, FileAction};
use crate::export;
use crate::images;
use crate::models::{DraftInput, ExportSelection, ReviewNote};
#[cfg(feature = "ai-workbench")]
use crate::revise_model;
use crate::theme;
use crate::validator;
#[cfg(feature = "ai-workbench")]
use crate::{models::GeneratedDraft, prompt};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::thread;

/// 模型原文 → 可落入正文的 Markdown：剥思考与围栏、按文种规整、补齐版式要素。
/// 所有 AI 产物（单次起草、润色、研究式起草）都走这一条，不另起一套。
#[cfg(feature = "ai-workbench")]
pub(crate) fn prepare_model_markdown(input: &DraftInput, raw: &str) -> String {
    let cleaned = prompt::sanitize_model_markdown(raw);
    let cleaned = crate::ai_services::citations::delivery_markdown(
        &cleaned,
        &input.research.bibliography_content,
    );
    let normalized = prompt::normalize_generated_markdown(input, &cleaned);
    export::finalize_markdown(input, &normalized)
}

/// 定稿后的确定性复查：要素校验与研究报告的锚点、引用检查。
#[cfg(feature = "ai-workbench")]
pub(crate) fn review_notes(
    input: &DraftInput,
    config: &crate::models::AppConfig,
    markdown: &str,
) -> Vec<ReviewNote> {
    validator::validate(input, markdown, &config.vocabulary, &config.security_rules)
        .into_iter()
        .map(ReviewNote::from)
        .chain(validator::research_anchor_notes(input, markdown))
        .chain(validator::research_citation_notes(input, markdown))
        .collect()
}

/// 不导出的 AI 产物收尾：定稿、复查、版式粗估，撞了输出上限再记一条。
/// 结果只进提案，由用户接受后才落入正文。
#[cfg(feature = "ai-workbench")]
pub(crate) fn reviewed_draft(
    input: &DraftInput,
    config: &crate::models::AppConfig,
    raw: &str,
    truncated: bool,
) -> GeneratedDraft {
    let markdown = prepare_model_markdown(input, raw);
    let title = export::document_title(input, &markdown);
    let mut warnings = review_notes(input, config, &markdown);
    warnings.extend(validator::estimate_layout_notes(&markdown));
    if truncated {
        warnings.push(ReviewNote::from(format!(
            "{}可在设置里调大「最大输出」后重新生成。",
            crate::ai_panel::TRUNCATED_NOTE
        )));
    }
    GeneratedDraft {
        markdown,
        title,
        warnings,
        proof_warnings: Vec::new(),
        proof_measured: false,
        files: Vec::new(),
    }
}

/// 生成的是调用时传入的正文与要素快照；旧成品不参与另存。
fn render_export_to(
    input: &DraftInput,
    markdown: &str,
    config: &crate::models::AppConfig,
    kind: ExportKind,
    target: &Path,
    protection: Option<&export::protection::Protection>,
) -> anyhow::Result<crate::app::widgets::ExportOutcome> {
    let memory_root = Path::new("memory-export");
    let mut selection = kind.selection(true, false);
    selection.print_trace = config.export.print_trace.clone();
    let (result, mut files) = mdx::memory_files::capture_outputs(|| {
        export_and_compile(
            memory_root,
            input,
            markdown,
            &selection,
            &config.vocabulary,
            &config.fonts,
            &config.numbering,
            &config.transmit_defaults,
            None,
            |_| {},
        )
    });
    let mut outcome = result?;
    if let Some(error) = &outcome.compile_error {
        anyhow::bail!("{error}");
    }
    let source = kind
        .find_in(&outcome.files)
        .ok_or_else(|| anyhow::anyhow!("未生成导出文件"))?;
    let bytes = files
        .remove(source)
        .ok_or_else(|| anyhow::anyhow!("内存中未生成所选格式"))?;
    let logical_target = if kind == ExportKind::Typst {
        target.with_extension("zip")
    } else {
        target.to_path_buf()
    };
    let target = export::protection::write_final_with(protection, &logical_target, &bytes)?;
    outcome.files = vec![target];
    Ok(outcome)
}

impl DraftPage<'_> {
    /// 发起一次后台任务：记在本篇名下并推进序号，结果只认这一对标识。
    pub(crate) fn begin_job(&mut self) -> (DocKey, u64) {
        self.doc.busy = true;
        self.doc.job_seq += 1;
        (self.doc.key, self.doc.job_seq)
    }

    /// 以右侧编辑框的当前内容为准导出，可以反复调用；这是“改稿—导出”的闭环出口。
    pub(crate) fn start_export_current(&mut self) {
        self.start_export_with(self.config.export.clone());
    }

    /// 「打印…」：更新一份 PDF，导完由外壳打开并弹打印对话框。
    /// 走的就是 [`Self::start_export_with`]，不另写一条导出路径。
    pub(crate) fn start_export_for_print(&mut self) {
        self.request_export(
            ExportKind::Pdf.selection(self.config.export.overwrite, self.config.export.receipt),
            None,
            true,
        );
    }

    /// 「输出」分区的 PDF / Word / Typst 包图标：只导这一种，导完由外壳打开
    /// （PDF 在应用内、Word 用系统程序、Typst 包在文件管理器里定位）。
    pub(crate) fn start_export_and_open(&mut self, kind: ExportKind) {
        self.request_export(
            kind.selection(self.config.export.overwrite, self.config.export.receipt),
            Some(kind),
            false,
        );
    }

    /// 按给定格式导出。「输出」分区里的「仅 Word」等入口用它临时只出一种格式，
    /// 不动设置页里勾好的常用格式。
    pub(crate) fn start_export_with(&mut self, selection: ExportSelection) {
        self.request_export(selection, None, false);
    }

    fn request_export(
        &mut self,
        mut selection: ExportSelection,
        open: Option<ExportKind>,
        print: bool,
    ) {
        selection.print_trace = self.config.export.print_trace.clone();
        if self.doc.busy || self.doc.generated_markdown.trim().is_empty() || !selection.any() {
            // 沿用原入口的错误提示，不为不会启动的任务弹风险框。
            self.start_confirmed_export(selection, open, print);
            return;
        }
        if print {
            self.start_confirmed_export(selection, None, true);
            return;
        }
        if crate::vault::key_handle().is_some() {
            self.actions.push(DraftAction::ChooseDraftDelivery {
                key: self.doc.key,
                selection,
                open,
                copy: None,
            });
            return;
        }
        {
            let key = self.doc.key;
            let mut formats = Vec::new();
            if selection.pdf {
                formats.push("PDF");
            }
            if selection.docx {
                formats.push("Word");
            }
            if selection.markdown {
                formats.push("Markdown 源码包");
            }
            if selection.typst {
                formats.push("Typst 源码包");
            }
            if selection.receipt {
                formats.push("随行单据");
            }
            let operation = format!(
                "导出当前稿件\n格式：{}\n输出目录：{}",
                formats.join("、"),
                self.config.output_dir,
            );
            let warning = super::super::app::output_guard::DOCUMENT_WARNING;
            self.actions.push(DraftAction::ConfirmPlaintextOutput {
                operation,
                warning,
                action: Box::new(move |app| {
                    if let Some(index) = app.doc_index_of_key(key) {
                        app.draft_page_at(index)
                            .start_confirmed_export(selection, open, print);
                    }
                }),
            });
        }
    }

    /// 仅由统一确认回调或未启用加密的入口调用，不持久化任何“已确认”状态。
    pub(crate) fn start_confirmed_export(
        &mut self,
        selection: ExportSelection,
        open: Option<ExportKind>,
        print: bool,
    ) {
        if self.doc.busy {
            return;
        }
        if self.doc.generated_markdown.trim().is_empty() {
            *self.status = "还没有可导出的内容：请先生成草稿，或直接在右侧粘贴稿件。".into();
            return;
        }
        if crate::storage::writes_blocked() {
            *self.status = "工作区正在锁定，已取消导出。".into();
            return;
        }
        if !selection.any() {
            *self.status = "请至少勾选一种导出格式。".into();
            return;
        }
        // 这里没有校验闸门，是想清楚了的：缺主送单位、名称不规范这类问题决定的是
        // 这份稿子能不能签发，不决定 PDF 能不能排出来——而看版式、请人过目，靠的
        // 恰恰就是这份 PDF。真正做不出成品的只有「正文为空」，上面那一条已经拦下了。
        // 要素问题照样要让人看见，所以弹开审校抽屉；具体条目由导出结束后的
        // revalidate 统一算，不必在这里先塞一份进去。
        if !validator::mustfix_issues(
            &self.doc.draft,
            &self.doc.generated_markdown,
            &self.config.vocabulary,
            &self.config.security_rules,
        )
        .is_empty()
        {
            self.doc.result_drawer_open = true;
        }
        // 成文日期只在导出这一刻查。编辑期间日期本来就该是旧的，那时候弹提示
        // 纯属打扰；而稿子做好放了几天才签发、日期还停在上周，是真出过的事。
        if let Some(message) = crate::proofread_rules::check_doc_date(
            &self.doc.draft,
            chrono::Local::now().date_naive(),
        ) {
            self.doc.warnings.push(ReviewNote::from(message));
            self.doc.result_drawer_open = true;
        }

        let protection = if print {
            None
        } else {
            match export::protection::Protection::current() {
                Ok(protection) => protection,
                Err(error) => {
                    *self.status = format!("无法导出：{error:#}");
                    return;
                }
            }
        };
        let (key, seq) = self.begin_job();
        *self.status = "正在导出当前审校稿…".into();
        self.doc.open_after_export = open;
        if print {
            *self.status = "正在内存中排版 PDF，排好后打开打印对话框…".into();
        } else if let Some(kind) = open {
            *self.status = format!("正在导出 {}，导完自动打开…", kind.label());
        }
        self.doc.output_files.clear();
        self.doc.export_error = None;
        let input = self.doc.draft.clone();
        let output_dir = PathBuf::from(&self.config.output_dir);
        let markdown = self.doc.generated_markdown.clone();
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
        let numbering = self.config.numbering;
        // 设置页「加密传输单」里的默认填写内容：传发任务单上照填。
        let transmit_defaults = self.config.transmit_defaults.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            if print {
                let (result, mut files) = mdx::memory_files::capture_outputs(|| {
                    export_and_compile(
                        Path::new("memory-print"),
                        &input,
                        &markdown,
                        &selection,
                        &vocabulary,
                        &fonts,
                        &numbering,
                        &transmit_defaults,
                        None,
                        |_| {},
                    )
                });
                let result = result
                    .and_then(|outcome| {
                        if let Some(error) = outcome.compile_error {
                            anyhow::bail!("{error}");
                        }
                        let path = ExportKind::Pdf
                            .find_in(&outcome.files)
                            .ok_or_else(|| anyhow::anyhow!("未生成打印 PDF"))?;
                        files
                            .remove(path)
                            .ok_or_else(|| anyhow::anyhow!("内存打印 PDF 不存在"))
                    })
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(WorkerResult::Doc {
                    key,
                    seq,
                    job: DocJob::MemoryPrintPdf(result),
                });
                return;
            }
            let render = || {
                export_and_compile(
                    &output_dir,
                    &input,
                    &markdown,
                    &selection,
                    &vocabulary,
                    &fonts,
                    &numbering,
                    &transmit_defaults,
                    None,
                    |message| {
                        let _ = tx.send(WorkerResult::Doc {
                            key,
                            seq,
                            job: DocJob::ExportProgress(message.into()),
                        });
                    },
                )
            };
            let result = if let Some(protection) = protection {
                let (result, files) = mdx::memory_files::capture_outputs(render);
                result.and_then(|mut outcome| {
                    let paths = protection.persist(files)?;
                    for path in &mut outcome.files {
                        *path = paths
                            .get(path)
                            .cloned()
                            .ok_or_else(|| anyhow::anyhow!("加密输出缺失：{}", path.display()))?;
                    }
                    for path in paths.into_values() {
                        if !outcome.files.contains(&path) {
                            outcome.files.push(path);
                        }
                    }
                    Ok(outcome)
                })
            } else {
                render()
            }
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::Exported(result),
            });
        });
    }

    pub(crate) fn start_media_export(
        &mut self,
        mut selection: ExportSelection,
        medium: crate::app::media_export::Medium,
        workspace: crate::vault::KeyHandle,
        wake: egui::Context,
    ) {
        if self.doc.busy || self.doc.generated_markdown.trim().is_empty() || !selection.any() {
            *self.status = "当前稿件没有可刻录的内容。".into();
            return;
        }
        selection.overwrite = true;
        let (key, seq) = self.begin_job();
        let input = self.doc.draft.clone();
        let markdown = self.doc.generated_markdown.clone();
        let config = self.config.clone();
        let tx = self.sender.clone();
        *self.status = "正在内存中生成刻录文件…".into();
        thread::spawn(move || {
            let result = (|| {
                use anyhow::{Context as _, ensure};
                crate::app::media_export::require_workspace(&workspace)?;
                let (result, captured) = mdx::memory_files::capture_outputs(|| {
                    export_and_compile(
                        Path::new("media-export"),
                        &input,
                        &markdown,
                        &selection,
                        &config.vocabulary,
                        &config.fonts,
                        &config.numbering,
                        &config.transmit_defaults,
                        None,
                        |_| {},
                    )
                });
                let outcome = result?;
                if let Some(error) = outcome.compile_error {
                    anyhow::bail!("生成 PDF 失败：{error}");
                }
                ensure!(!captured.is_empty(), "未生成任何刻录文件");
                let mut files = std::collections::BTreeMap::new();
                for (path, data) in captured {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .context("刻录文件名无效")?;
                    ensure!(
                        files.insert(name.to_owned(), data).is_none(),
                        "刻录文件同名：{name}"
                    );
                }
                let count = files.len();
                let image = zeroize::Zeroizing::new(crate::optical_image::build(files)?);
                let _ = tx.send(WorkerResult::Doc {
                    key,
                    seq,
                    job: DocJob::ExportProgress("正在刻录并回读校验…".into()),
                });
                wake.request_repaint();
                medium.burn(&workspace, &image)?;
                Ok::<_, anyhow::Error>(count)
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::MediaExported(result),
            });
            wake.request_repaint();
        });
    }

    /// 发起一轮小模型逐句文字复核。
    ///
    /// 与起草、优化的根本区别：它**不产出正文**，只产出待确认的建议。所以既不
    /// 需要事实闸门那一套（没有整篇改写可言），也不会触发自动导出——复核结果
    /// 在用户逐条点过之前，正文一个字都没变。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn start_model_review(&mut self) {
        if self.doc.busy {
            return;
        }
        if !self.config.revise_model.enabled {
            *self.status =
                "文字复核未启用：请先在「AI 管理 → 模型服务」配置复核用的小模型。".into();
            return;
        }
        let markdown = self.doc.generated_markdown.clone();
        if markdown.trim().is_empty() {
            *self.status = "还没有可复核的正文。".into();
            return;
        }
        let cfg = self.config.revise_model.clone();
        let tasks = cfg.enabled_tasks();
        if tasks.is_empty() {
            *self.status = "没有启用任何检查器：请在「AI 管理 → 文字复核」中至少开启一项。".into();
            return;
        }
        let sentences = revise_model::segment_sentences(&markdown, cfg.max_sentence_chars);
        if sentences.is_empty() {
            *self.status = "正文里没有可逐句复核的句子（标题、表格和过短的句子不送检）。".into();
            return;
        }

        let (key, seq) = self.begin_job();
        // 调用次数是句数乘检查器数，说清楚才不会让人以为卡住了。
        *self.status = format!(
            "正在逐句复核：{} 句 × {} 个检查器…",
            sentences.len().min(cfg.max_sentences),
            tasks.len()
        );
        let model = match self.config.revise_chat(false) {
            Ok(model) => model,
            Err(error) => {
                *self.status = error.to_string();
                return;
            }
        };
        let lexicon = crate::proofread::Lexicon::resolved(&self.config.proofread);
        let vocabulary = self.config.vocabulary.clone();
        let cache = self.doc.revise_cache.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let progress = {
                let tx = tx.clone();
                move |done: usize, total: usize| {
                    let _ = tx.send(WorkerResult::Doc {
                        key,
                        seq,
                        job: DocJob::ExportProgress(format!("正在逐句复核 {done}/{total}…")),
                    });
                }
            };
            let result = revise_model::review(
                revise_model::ReviewRequest {
                    cfg: &cfg,
                    model: &model,
                    lexicon: &lexicon,
                    vocabulary: &vocabulary,
                    markdown: &markdown,
                    cache: &cache,
                    tasks: &tasks,
                },
                &progress,
            )
            .map_err(|error: anyhow::Error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::Reviewed(result),
            });
        });
    }

    /// 「输出目录」要定位的成品：本篇这次运行里最近一次导出的文件优先，
    /// 其次是导出目录里扫到的本篇最近成品（PDF、Word、Typst 包依次取）。
    pub(crate) fn latest_output_file(&self) -> Option<PathBuf> {
        self.doc
            .output_files
            .iter()
            .find(|file| file.is_file())
            .cloned()
            .or_else(|| {
                ExportKind::ALL
                    .into_iter()
                    .find_map(|kind| self.export_links.path(kind))
                    .map(Path::to_path_buf)
            })
    }

    /// 在文件管理器里选中本篇最近导出的成品；还没导出过就打开输出目录本身。
    pub(crate) fn reveal_latest_output(&mut self) {
        match self.latest_output_file() {
            Some(path) => self.run_file_action(FileAction::Reveal(path)),
            None => self.open_output_dir(),
        }
    }

    pub(crate) fn open_output_dir(&mut self) {
        crate::app::widgets::open_output_dir(&self.config.output_dir, self.status);
    }

    /// 成品入口：PDF / Word / Typst 包。单击按当前审校稿只导这一种并打开，
    /// 不必先导出再另找按钮打开。右键可复制、定位已有成品，或重新生成并导出到别处。
    pub(crate) fn export_open_buttons(&mut self, ui: &mut egui::Ui) {
        // 导出目录里同一文稿的文件夹都以“去掉时间戳的导出主干”为前缀，
        // 用它过滤出属于当前文稿的目录，避免打开别的文稿的成品。
        let stem = export::document_stem_prefix(
            &self.doc.draft,
            &export::document_title(&self.doc.draft, &self.doc.generated_markdown),
        );
        self.export_links
            .refresh(&self.config.output_dir, Some(&stem));
        let ready = !self.doc.busy && !self.doc.generated_markdown.trim().is_empty();
        let mut action = None;
        let mut export = None;
        for kind in ExportKind::ALL {
            let path = self.export_links.path(kind).map(Path::to_path_buf);
            let label = kind.label();
            let tint = if ready { accent() } else { theme::text_muted() };
            let response = ui.add_enabled(
                ready,
                egui::Button::image(kind.icon().image_sized(16.0).tint(tint))
                    .image_tint_follows_text_color(false)
                    .min_size(egui::vec2(34.0, 26.0))
                    .corner_radius(theme::chrome_radius(6)),
            );
            let opens = match kind {
                ExportKind::Typst => "在文件管理器里定位",
                _ => "打开",
            };
            let tip = format!("按当前审校稿导出 {label} 并{opens}，不改设置里勾好的常用格式");
            let response = response
                .on_hover_text(tip.as_str())
                .on_disabled_hover_text(tip.as_str());
            if response.clicked() {
                export = Some(kind);
            }
            response.context_menu(|ui| {
                if ui
                    .add_enabled(
                        path.is_some(),
                        theme::menu_item(theme::Icon::Copy, "复制文件"),
                    )
                    .clicked()
                {
                    action = Some(FileAction::CopyToClipboard(path.clone().unwrap()));
                    ui.close();
                }
                if ui
                    .add(theme::menu_item(theme::Icon::FileDown, "导出到…"))
                    .clicked()
                {
                    action = Some(FileAction::SaveAs(kind));
                    ui.close();
                }
                if ui
                    .add_enabled(
                        path.is_some(),
                        theme::menu_item(theme::Icon::Reveal, "在文件管理器中定位"),
                    )
                    .clicked()
                {
                    action = Some(FileAction::Reveal(path.clone().unwrap()));
                    ui.close();
                }
            });
        }
        if let Some(action) = action {
            self.run_file_action(action);
        }
        if let Some(kind) = export {
            self.start_export_and_open(kind);
        }
    }

    pub(crate) fn run_file_action(&mut self, action: FileAction) {
        let (result, path, verb) = match action {
            FileAction::Reveal(path) => (reveal_in_os(&path), path, "定位"),
            FileAction::CopyToClipboard(path) => {
                self.actions.push(DraftAction::CopyFile { path });
                return;
            }
            FileAction::SaveAs(kind) => {
                self.save_export_copy(kind);
                return;
            }
        };
        match result {
            Ok(()) => {
                *self.status = format!(
                    "已{verb} {}。",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("文件")
                )
            }
            Err(error) => *self.status = format!("{verb}失败：{error}"),
        }
    }

    /// 右键“导出到…”：先选位置，再从当前正文和表单重新生成。
    fn save_export_copy(&mut self, kind: ExportKind) {
        if self.doc.busy || self.doc.generated_markdown.trim().is_empty() {
            return;
        }
        let title = export::document_title(&self.doc.draft, &self.doc.generated_markdown);
        let name = match export::suggested_file_name(&self.doc.draft, &title, kind) {
            Ok(name) => name,
            Err(error) => {
                *self.status = format!("无法导出：{error:#}");
                return;
            }
        };
        self.actions.push(DraftAction::SaveExportCopy {
            key: self.doc.key,
            kind,
            name,
        });
    }

    /// 由应用层单次明文输出确认和异步文件框回调进入；后台重新生成，绝不复制旧缓存。
    pub(crate) fn export_current_to(
        &mut self,
        kind: ExportKind,
        target: PathBuf,
        protection: Option<export::protection::Protection>,
    ) {
        if self.doc.busy || self.doc.generated_markdown.trim().is_empty() {
            return;
        }
        let input = self.doc.draft.clone();
        let markdown = self.doc.generated_markdown.clone();
        let config = self.config.clone();
        let (key, seq) = self.begin_job();
        self.doc.open_after_export = None;
        self.doc.output_files.clear();
        self.doc.export_error = None;
        *self.status = "正在按当前内容重新导出…".into();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = render_export_to(
                &input,
                &markdown,
                &config,
                kind,
                &target,
                protection.as_ref(),
            )
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::Exported(result),
            });
        });
    }

    /// 功能区“导入文档”：选一个现成文档，转成 markdown 插到编辑器光标处。
    ///
    /// 文件框由应用层在后台线程开（主线程不能等），这里只把请求交出去。
    pub(crate) fn import_document(&mut self, _ctx: &egui::Context) {
        if self.doc.read_only() {
            return;
        }
        self.actions
            .push(DraftAction::ImportDocument { key: self.doc.key });
    }

    /// 把用户选中的文档转成 markdown 插到编辑器光标处。
    ///
    /// 沿用主线程同步转换：本机短文档样张提取耗时为毫秒级（PDF 约 4–12ms）。
    /// 这不是大文件的耗时上限，长 PDF 仍可能让界面短暂等待。
    pub(crate) fn import_document_from(&mut self, ctx: &egui::Context, path: &Path) {
        if self.doc.read_only() {
            return;
        }
        // 稿子里已经有 `# 标题` 就不再从导入的正文里认标题，免得正文区出现两个。
        let detect_title = !self
            .doc
            .generated_markdown
            .lines()
            .any(|line| line.trim_start().starts_with("# "));
        match doc_import::to_draft_markdown(path, detect_title) {
            Ok(imported) => {
                self.insert_imported_markdown(ctx, &imported.markdown, path);
                self.status.push_str(&imported.images.notice());
            }
            Err(error) => *self.status = format!("导入失败：{error:#}"),
        }
    }

    /// 功能区“插入图片”：选 png/jpg/pdf 等文件，复制入库后把 markdown 图片引用
    /// 插到编辑器光标处，多文件按行分隔。
    ///
    /// 图片宽度不写进 markdown，预览与导出统一按页面（版心）宽度等比缩放。
    pub(crate) fn insert_images(&mut self, _ctx: &egui::Context) {
        if self.doc.read_only() {
            return;
        }
        self.actions
            .push(DraftAction::InsertImages { key: self.doc.key });
    }

    /// 把用户选中的图片复制入库，并把 markdown 图片引用插到编辑器光标处。
    pub(crate) fn insert_images_from(&mut self, ctx: &egui::Context, files: &[PathBuf]) {
        if self.doc.read_only() || files.is_empty() {
            return;
        }
        match images::import(files) {
            Ok(imported) if imported.is_empty() => {
                *self.status =
                    "没有可插入的图片文件（支持 PNG / JPG / WebP / BMP / GIF / PDF）。".to_string();
            }
            Ok(imported) => {
                let markdown = imported
                    .iter()
                    .map(|image| image.markdown.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.insert_imported_markdown(ctx, &markdown, &files[0]);
                let first = imported[0].rel_path.rsplit('/').next().unwrap_or_default();
                *self.status = if imported.len() == 1 {
                    format!("已插入图片 {first}。")
                } else {
                    format!("已插入 {} 张图片，第一张为 {first}。", imported.len())
                };
            }
            Err(error) => *self.status = format!("插入图片失败：{error:#}"),
        }
    }

    /// 把导入的 markdown 插进审校稿，并立即重新校验一遍。
    pub(crate) fn insert_imported_markdown(
        &mut self,
        ctx: &egui::Context,
        markdown: &str,
        path: &Path,
    ) {
        self.insert_block(ctx, markdown);
        self.revalidate();
        *self.status = format!(
            "已从 {} 导入 {} 字。",
            doc_import::file_label(path),
            markdown.chars().count()
        );
        self.status.push_str(doc_import::import_notice(path));
        if !self.doc.warnings.is_empty() || !self.doc.revisions.is_empty() {
            self.open_result_drawer();
        }
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use crate::models::TemplateKind;

    #[test]
    fn captured_exports_leave_no_document_directory_or_hidden_export_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let virtual_root = dir.path().join("must-not-exist");
        let config = crate::models::AppConfig::default();
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            title_hint: "不应出现在磁盘目录的标题".into(),
            ..Default::default()
        };
        for kind in ExportKind::ALL {
            // PDF 要随包字体；没有 runtime/fonts 的环境（CI）排不了版，
            // 只跑其余两种格式——「内存导出不在磁盘落目录」在 Word 与 Typst 包上照样成立。
            if kind == ExportKind::Pdf && crate::portable_runtime::find_font_dir().is_none() {
                continue;
            }
            let (result, files) = mdx::memory_files::capture_outputs(|| {
                export_and_compile(
                    &virtual_root,
                    &input,
                    "# 内存测试标题\n\n内存生成的正文。",
                    &kind.selection(true, false),
                    &config.vocabulary,
                    &config.fonts,
                    &config.numbering,
                    &config.transmit_defaults,
                    None,
                    |_| {},
                )
            });
            let outcome = result.unwrap();
            assert!(outcome.compile_error.is_none());
            let path = kind.find_in(&outcome.files).unwrap();
            assert!(!files[path].is_empty());
            assert!(
                !virtual_root.exists(),
                "连隐藏导出清单或空的业务目录也不能落盘"
            );
        }
    }

    #[test]
    fn export_to_regenerates_current_body_and_elements_and_replaces_target() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("最新.docx");
        std::fs::write(&target, b"old cached file").unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.department_code = "某办函".into();
        input.profile.document_number = "99".into();
        let outcome = render_export_to(
            &input,
            "# 最新标题\n\n这是修改后的最新正文。",
            &crate::models::AppConfig::default(),
            ExportKind::Word,
            &target,
            None,
        )
        .unwrap();
        assert_eq!(outcome.files, vec![target.clone()]);
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&target).unwrap()).unwrap();
        let mut xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("这是修改后的最新正文。"));
        assert!(xml.contains("某办函"));
        assert!(xml.contains("99"));
        // 指定位置只留下所选成品，不遗留额外目录、源码包或清单。
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    #[cfg(feature = "ai-workbench")]
    fn proposal_delivery_removes_internal_marks_and_preserves_research_syntax() {
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..DraftInput::default()
        };
        input.research.bibliography_content = "@book{K2, title = {真实文献}}".into();
        let body = concat!(
            "# 研究报告\n\n<!-- [正文] -->\n\n## 背景 {#chap:bg}\n\n",
            "已有结论，见第{@chap:bg}章[@K2]。补充说明[^n]:(来源说明)。\n\n",
            "$x^2$，尚需确认【待核实：统计口径】。\n"
        );
        let ai = body.replace("已有结论", "已有结论[@K1][K3; K4]【K5；6】");
        let draft = reviewed_draft(&input, &crate::models::AppConfig::default(), &ai, false);
        assert_eq!(draft.markdown, prepare_model_markdown(&input, body));
        assert!(ai.contains("[@K1]"), "AI 原稿保留来源标识");
        assert!(
            draft
                .warnings
                .iter()
                .any(|note| note.message.contains("待核实"))
        );
        assert!(
            !draft
                .warnings
                .iter()
                .any(|note| note.message.contains("[@K1]"))
        );
    }

    #[test]
    #[cfg(feature = "ai-workbench")]
    fn non_research_proposals_also_remove_internal_source_marks() {
        let input = DraftInput::default();
        let body = "# 工作通知\n\n各单位加强巡查。";
        assert_eq!(
            prepare_model_markdown(&input, "# 工作通知\n\n各单位加强巡查[@K2][K3; K4]。"),
            prepare_model_markdown(&input, body)
        );
    }
}
