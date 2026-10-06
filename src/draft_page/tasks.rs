//! 后台任务与导入导出：生成/优化/导出/文件导入。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::tasks`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::{
    DocJob, DraftAction, WorkerResult, accent, export_and_compile, open_in_os, reveal_in_os,
};
use crate::doc_import;
use crate::draft_page::{DocKey, DraftPage, ExportKind, FileAction};
use crate::export;
use crate::file_clipboard;
use crate::images;
use crate::models::{DraftInput, ExportSelection, GeneratedDraft, ReviewNote};
use crate::prompt;
use crate::revise_model;
use crate::theme;
use crate::validator;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::thread;

/// 模型原文 → 可落入正文的 Markdown：剥思考与围栏、按文种规整、补齐版式要素。
/// 所有 AI 产物（单次起草、润色、研究式起草）都走这一条，不另起一套。
pub(crate) fn prepare_model_markdown(input: &DraftInput, raw: &str) -> String {
    let cleaned = prompt::sanitize_model_markdown(raw);
    let normalized = prompt::normalize_generated_markdown(input, &cleaned);
    export::finalize_markdown(input, &normalized)
}

/// 定稿后的确定性复查：要素校验与研究报告的锚点、引用检查。
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

    /// 按给定格式导出。「输出」分区里的「仅 Word」等入口用它临时只出一种格式，
    /// 不动设置页里勾好的常用格式。
    pub(crate) fn start_export_with(&mut self, selection: ExportSelection) {
        if self.doc.busy {
            return;
        }
        if self.doc.generated_markdown.trim().is_empty() {
            *self.status = "还没有可导出的内容：请先生成草稿，或直接在右侧粘贴稿件。".into();
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

        // 在主线程按本次实际导出的快照认定版本；不能只用“已保存”或最新版本名称。
        let version_name = match (self.doc.manuscript_id, self.store.as_deref_mut()) {
            (Some(id), Some(store)) => {
                let selected_version = self
                    .doc
                    .loaded_version
                    .as_ref()
                    .filter(|loaded| loaded.manuscript_id == id)
                    .map(|loaded| loaded.version_number);
                match store.export_version_name(
                    id,
                    selected_version,
                    &self.doc.draft,
                    &self.doc.generated_markdown,
                ) {
                    Ok(name) => name,
                    Err(error) => {
                        *self.status = format!("读取导出版本失败：{error:#}");
                        return;
                    }
                }
            }
            _ => None,
        };
        let (key, seq) = self.begin_job();
        *self.status = "正在导出当前审校稿…".into();
        self.doc.output_files.clear();
        self.doc.export_error = None;
        let input = self.doc.draft.clone();
        let output_dir = PathBuf::from(&self.config.output_dir);
        let markdown = self.doc.generated_markdown.clone();
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
        let numbering = self.config.numbering;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = export_and_compile(
                &output_dir,
                &input,
                &markdown,
                &selection,
                &vocabulary,
                &fonts,
                &numbering,
                version_name.as_deref(),
                |message| {
                    let _ = tx.send(WorkerResult::Doc {
                        key,
                        seq,
                        job: DocJob::ExportProgress(message.into()),
                    });
                },
            )
            .map_err(|error: anyhow::Error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::Exported(result),
            });
        });
    }

    /// 发起一轮小模型逐句文字复核。
    ///
    /// 与起草、优化的根本区别：它**不产出正文**，只产出待确认的建议。所以既不
    /// 需要事实闸门那一套（没有整篇改写可言），也不会触发自动导出——复核结果
    /// 在用户逐条点过之前，正文一个字都没变。
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

    pub(crate) fn open_output_dir(&mut self) {
        let dir = PathBuf::from(self.config.output_dir.trim());
        if dir.as_os_str().is_empty() {
            *self.status = "尚未设置输出目录。".into();
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&dir) {
            *self.status = format!("无法创建输出目录：{error}");
            return;
        }
        match open_in_os(&dir) {
            Ok(()) => *self.status = format!("已打开输出目录 {}。", dir.display()),
            Err(error) => *self.status = format!("打开输出目录失败：{error}"),
        }
    }

    /// 仿 WinEdt 的成品入口：TEX / PDF / WORD 三枚，当前文稿的导出目录里有
    /// 对应成品才点亮，点开的是当前文稿最近一次导出（属于它的子目录中修改时间
    /// 最新）留下的那一份。右键可复制文件到剪贴板、另存到别处或在文件管理器里定位。
    pub(crate) fn export_open_buttons(&mut self, ui: &mut egui::Ui) {
        // 导出目录里同一文稿的文件夹都以“去掉时间戳的导出主干”为前缀，
        // 用它过滤出属于当前文稿的目录，避免打开别的文稿的成品。
        let stem = export::document_stem_prefix(
            &self.doc.draft,
            &export::document_title(&self.doc.draft, &self.doc.generated_markdown),
        );
        self.export_links
            .refresh(&self.config.output_dir, Some(&stem));
        let mut action = None;
        for kind in ExportKind::ALL {
            let path = self.export_links.path(kind).map(Path::to_path_buf);
            let lit = path.is_some();
            let label = kind.label();
            let tint = if lit { accent() } else { theme::text_muted() };
            let response = ui.add_enabled(
                lit,
                egui::Button::image(kind.icon().image_sized(16.0).tint(tint))
                    .image_tint_follows_text_color(false)
                    .min_size(egui::vec2(34.0, 26.0))
                    .corner_radius(egui::CornerRadius::same(6)),
            );
            let Some(path) = path else {
                response.on_hover_text(format!("导出目录里还没有 {label} 文件"));
                continue;
            };
            let response =
                response.on_hover_text(format!("打开最近导出的 {label}：{}", path.display()));
            response.context_menu(|ui| {
                if ui
                    .add(theme::menu_item(theme::Icon::Copy, "复制文件"))
                    .clicked()
                {
                    action = Some(FileAction::CopyToClipboard(path.clone()));
                    ui.close();
                }
                if ui
                    .add(theme::menu_item(theme::Icon::FileDown, "导出到…"))
                    .clicked()
                {
                    action = Some(FileAction::SaveAs(path.clone()));
                    ui.close();
                }
                if ui
                    .add(theme::menu_item(theme::Icon::Reveal, "在文件管理器中定位"))
                    .clicked()
                {
                    action = Some(FileAction::Reveal(path.clone()));
                    ui.close();
                }
            });
            if response.clicked() {
                action = Some(FileAction::Open(path));
            }
        }
        if let Some(action) = action {
            self.run_file_action(action);
        }
    }

    pub(crate) fn run_file_action(&mut self, action: FileAction) {
        if let FileAction::Open(path) = &action
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            self.actions.push(DraftAction::OpenPdf(path.clone()));
            *self.status = format!(
                "已在应用内打开 {}。",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("PDF")
            );
            return;
        }
        let (result, path, verb) = match action {
            FileAction::Open(path) => (open_in_os(&path), path, "打开"),
            FileAction::Reveal(path) => (reveal_in_os(&path), path, "定位"),
            FileAction::CopyToClipboard(path) => {
                (file_clipboard::copy_file(&path), path, "复制到剪贴板")
            }
            FileAction::SaveAs(path) => {
                self.save_export_copy(&path);
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

    /// 右键“导出到…”：弹出保存框，把这份成品另存一份到用户选的位置。
    fn save_export_copy(&mut self, source: &Path) {
        let file_name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut dialog = rfd::FileDialog::new().set_file_name(&file_name);
        if let Some(extension) = source.extension().and_then(|extension| extension.to_str()) {
            let label = format!("{} 文件", extension.to_ascii_uppercase());
            dialog = dialog.add_filter(label, &[extension]);
        }
        let Some(target) = dialog.save_file() else {
            return;
        };
        let same = match (source.canonicalize(), target.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if same {
            *self.status = "目标就是原文件，未做改动。".into();
            return;
        }
        *self.status = match std::fs::copy(source, &target) {
            Ok(_) => format!("已导出到 {}。", target.display()),
            Err(error) => format!("导出到 {} 失败：{error}", target.display()),
        };
    }

    /// 功能区“导入文档”：选一个现成文档，转成 markdown 插到编辑器光标处。
    ///
    /// 沿用主线程同步转换：本机短文档样张提取耗时为毫秒级（PDF 约 4–12ms）。
    /// 这不是大文件的耗时上限，长 PDF 仍可能让界面短暂等待。
    pub(crate) fn import_document(&mut self, ctx: &egui::Context) {
        if self.doc.read_only() {
            return;
        }
        let Some(path) = doc_import::pick_file() else {
            return;
        };
        match doc_import::to_markdown(&path) {
            Ok(markdown) => self.insert_imported_markdown(ctx, &markdown, &path),
            Err(error) => *self.status = format!("导入失败：{error:#}"),
        }
    }

    /// 功能区“插入图片”：选 png/jpg/pdf 等文件，复制入库后把 markdown 图片引用
    /// 插到编辑器光标处，多文件按行分隔。
    ///
    /// 图片宽度不写进 markdown，预览与导出统一按页面（版心）宽度等比缩放。
    pub(crate) fn insert_images(&mut self, ctx: &egui::Context) {
        if self.doc.read_only() {
            return;
        }
        let Some(files) = images::pick_files() else {
            return;
        };
        match images::import(&files) {
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
