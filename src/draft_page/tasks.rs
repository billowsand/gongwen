//! 后台任务与导入导出：生成/优化/导出/文件导入。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::tasks`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::{
    DocJob, DraftAction, WorkerResult, accent, export_and_compile, open_in_os, reveal_in_os,
};
use crate::doc_import;
use crate::draft_page::{AiTaskRequest, AiWorkflowKind, DocKey, DraftPage, ExportKind, FileAction};
use crate::export;
use crate::images;
use crate::lmstudio;
use crate::models::{DraftInput, ExportSelection, GeneratedDraft, ReviewNote, TemplateKind};
use crate::prompt;
use crate::rag;
use crate::storage;
use crate::theme;
use crate::validator;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::thread;

/// 起草时检索知识库并把结果拼成提示词参考节。检索失败降级为空串，不阻塞
/// 起草——RAG 是增强而非硬依赖；但降级原因会随 `notes` 回给调用方显示，
/// 不再是只有翻服务端日志才知道的静默失败。
///
/// 返回 (参考节, 给用户看的说明)。
pub(crate) fn retrieve_reference(
    rag_cfg: &crate::models::RagConfig,
    chat: &crate::models::LmStudioConfig,
    input: &DraftInput,
    instruction: &str,
    kind_filter: Option<TemplateKind>,
) -> (String, Vec<String>) {
    let query = if input.title_hint.trim().is_empty() {
        instruction.trim().to_string()
    } else {
        format!("{}\n{}", input.title_hint.trim(), instruction.trim())
    };
    if query.trim().is_empty() {
        return (String::new(), Vec::new());
    }
    let db_path = match storage::manuscript_db_path() {
        Ok(path) => path,
        Err(error) => return (String::new(), vec![format!("知识库路径不可用：{error:#}")]),
    };
    let outcome = match rag::retrieve(rag_cfg, chat, &db_path, &query, kind_filter) {
        Ok(outcome) => outcome,
        Err(error) => {
            return (
                String::new(),
                vec![format!("知识库检索失败，本次未注入参考：{error:#}")],
            );
        }
    };
    let mut notes = outcome.warnings;
    if outcome.chunks.is_empty() {
        notes.push("知识库没有检索到相关片段，本次未注入参考。".into());
        return (String::new(), notes);
    }
    let refs: Vec<prompt::ReferenceChunk> = outcome
        .chunks
        .into_iter()
        .map(|chunk| prompt::ReferenceChunk {
            kind_label: chunk.kind.label().to_string(),
            doc_title: chunk.doc_title,
            section: chunk.section,
            text: chunk.text,
        })
        .collect();
    notes.push(format!("已注入 {} 段知识库参考。", refs.len()));
    (prompt::format_reference_section(&refs), notes)
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
        let blockers = validator::blocking_issues(
            &self.doc.draft,
            &self.doc.generated_markdown,
            &self.config.vocabulary,
            &self.config.security_rules,
        );
        if !blockers.is_empty() {
            self.doc.warnings.extend(
                blockers
                    .iter()
                    .map(|message| ReviewNote::from(format!("阻断导出：{message}"))),
            );
            self.doc.warnings.sort_by(|a, b| a.message.cmp(&b.message));
            self.doc.warnings.dedup_by(|a, b| a.message == b.message);
            self.doc.result_drawer_open = true;
            *self.status = format!(
                "正式导出已暂停：还有 {} 项硬错误，请在审校结果中处理后重试。",
                blockers.len()
            );
            return;
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

        let (key, seq) = self.begin_job();
        *self.status = "正在导出当前审校稿…".into();
        self.doc.output_files.clear();
        self.doc.export_error = None;
        let input = self.doc.draft.clone();
        let output_dir = PathBuf::from(&self.config.output_dir);
        let markdown = self.doc.generated_markdown.clone();
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = export_and_compile(
                &output_dir,
                &input,
                &markdown,
                &selection,
                &vocabulary,
                &fonts,
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

    /// 兼容旧提示词选择面板；新入口统一走 [`start_ai_task`]。
    pub(crate) fn start_optimize(&mut self, instruction: String, label: String) {
        let current_empty = self.doc.generated_markdown.trim().is_empty();
        self.start_ai_task(AiTaskRequest {
            kind: if current_empty {
                AiWorkflowKind::Material
            } else {
                AiWorkflowKind::Polish
            },
            label,
            material: instruction.clone(),
            instruction,
            baseline: String::new(),
            use_rag: current_empty && self.doc.use_knowledge_rag && self.config.rag.enabled,
            review_before_apply: !current_empty,
        });
    }

    /// 工作台确认后的统一 AI 执行入口。任务类型由用户明确选择，不再根据编辑框
    /// 是否为空猜测；已有内容上的结果一律进入修改提案。
    pub(crate) fn start_ai_task(&mut self, request: AiTaskRequest) {
        if self.doc.busy {
            return;
        }
        let current = self.doc.generated_markdown.trim().to_string();
        let drafting = request.kind != AiWorkflowKind::Polish;
        if drafting && request.material.trim().is_empty() && request.baseline.trim().is_empty() {
            *self.status = "请先提供已确认的材料或选择一篇基准稿。".into();
            return;
        }
        if !drafting && current.is_empty() {
            *self.status = "当前没有可润色的审校稿。".into();
            return;
        }
        // 需要人工审阅的结果不能在接受前自动导出。
        let export_now = self.config.auto_export && !request.review_before_apply;
        if export_now && !self.config.export.any() {
            *self.status = "已勾选“完成后自动导出”，请先在设置里选择至少一种导出格式。".into();
            return;
        }

        let time_context = prompt::TimeContext::now();
        if self.doc.draft.date_is_auto {
            self.doc.draft.date = time_context.today.clone();
        }
        self.config.upsert_profile(self.doc.draft.profile.clone());
        let (key, seq) = self.begin_job();
        *self.status = if request.use_rag {
            format!("正在按“{}”检索并起草…", request.label)
        } else if drafting {
            format!("正在按“{}”起草…", request.label)
        } else {
            format!("正在按“{}”生成受控修改提案…", request.label)
        };
        self.doc.ai_prompt_last_label = request.label.clone();
        self.doc.ai_review_baseline = request.review_before_apply.then(|| current.clone());
        self.doc.ai_proposal = None;
        self.doc.output_files.clear();
        self.doc.export_error = None;
        // 记住这次用的提示词，重启后选择面板仍能标出“上次使用”。
        let _ = storage::save(self.config);

        let input = self.doc.draft.clone();
        let config = self.config.clone();
        let selection = self.config.export.clone();
        let tx = self.sender.clone();
        let use_rag = request.use_rag && self.config.rag.enabled;
        // 文种过滤：`RagKindFilter::Follow` 跟随当前文种，`All` 不限文种。
        let rag_kind = self.doc.rag_kind_filter.resolve(self.doc.draft.kind);
        let rag_cfg = self.config.rag.clone();
        let workflow = request.kind;
        let instruction = request.instruction;
        let material = request.material;
        let baseline = request.baseline;
        let review_before_apply = request.review_before_apply;
        thread::spawn(move || {
            let result = (|| {
                let system = prompt::build_system_prompt(&time_context);
                let user = if drafting {
                    let reference = if use_rag {
                        let _ = tx.send(WorkerResult::Doc {
                            key,
                            seq,
                            job: DocJob::ExportProgress("正在检索知识库…".into()),
                        });
                        let (reference, notes) = retrieve_reference(
                            &rag_cfg,
                            &config.lm_studio,
                            &input,
                            &material,
                            rag_kind,
                        );
                        // 检索的降级说明要让用户看见，不能只留在服务端日志里。
                        if !notes.is_empty() {
                            let _ = tx.send(WorkerResult::Doc {
                                key,
                                seq,
                                job: DocJob::ExportProgress(format!(
                                    "知识库：{}",
                                    notes.join("　")
                                )),
                            });
                        }
                        reference
                    } else {
                        String::new()
                    };
                    if workflow == AiWorkflowKind::Similar {
                        prompt::build_similar_prompt(
                            &input,
                            &config.vocabulary,
                            &baseline,
                            &instruction,
                            &material,
                        )
                    } else {
                        prompt::build_draft_prompt(
                            &input,
                            &config.vocabulary,
                            &material,
                            &reference,
                        )
                    }
                } else {
                    let protected =
                        crate::ai_guard::protected_facts_prompt(&current, &config.vocabulary);
                    prompt::build_optimize_prompt(
                        &input,
                        &current,
                        &format!("{}{}", instruction.trim(), protected),
                    )
                };
                let raw = lmstudio::generate(&config.lm_studio, &system, &user)?;
                let cleaned = prompt::sanitize_model_markdown(&raw);
                let normalized = prompt::normalize_generated_markdown(&input, &cleaned);
                let markdown = export::finalize_markdown(&input, &normalized);
                let title = export::extract_title(&markdown, &input.title_hint);
                let mut warnings: Vec<ReviewNote> = validator::validate(
                    &input,
                    &markdown,
                    &config.vocabulary,
                    &config.security_rules,
                )
                .into_iter()
                .map(ReviewNote::from)
                .collect();
                let mut proof_warnings: Vec<ReviewNote> = Vec::new();
                let mut proof_measured = false;
                let estimated = validator::estimate_layout_notes(&markdown);
                let blockers = validator::blocking_issues(
                    &input,
                    &markdown,
                    &config.vocabulary,
                    &config.security_rules,
                );
                let files = if export_now && blockers.is_empty() {
                    let outcome = export_and_compile(
                        PathBuf::from(&config.output_dir).as_path(),
                        &input,
                        &markdown,
                        &selection,
                        &config.vocabulary,
                        &config.fonts,
                        |message| {
                            let _ = tx.send(WorkerResult::Doc {
                                key,
                                seq,
                                job: DocJob::ExportProgress(message.into()),
                            });
                        },
                    )?;
                    warnings.extend(outcome.warnings.into_iter().map(ReviewNote::from));
                    proof_warnings = outcome.proof_warnings;
                    proof_measured = outcome.proof_measured;
                    outcome.files
                } else {
                    if export_now {
                        warnings.extend(
                            blockers
                                .into_iter()
                                .map(|message| ReviewNote::from(format!("阻断导出：{message}"))),
                        );
                    }
                    vec![]
                };
                if !proof_measured {
                    warnings.extend(estimated);
                }
                Ok(GeneratedDraft {
                    markdown,
                    title,
                    warnings,
                    proof_warnings,
                    proof_measured,
                    files,
                })
            })()
            .map_err(|error: anyhow::Error| format!("{error:#}"));
            let job = if review_before_apply {
                DocJob::Proposed(result)
            } else if drafting {
                DocJob::Drafted(result)
            } else {
                DocJob::Optimized(result)
            };
            let _ = tx.send(WorkerResult::Doc { key, seq, job });
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
    /// 最新）留下的那一份。右键可在文件管理器里定位。
    pub(crate) fn export_open_buttons(&mut self, ui: &mut egui::Ui) {
        // 导出目录里同一文稿的文件夹都以“去掉时间戳的导出主干”为前缀，
        // 用它过滤出属于当前文稿的目录，避免打开别的文稿的成品。
        let stem = export::document_stem_prefix(
            &self.doc.draft,
            &export::extract_title(&self.doc.generated_markdown, &self.doc.draft.title_hint),
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
                egui::Button::image(kind.icon().image_sized(20.0).tint(tint))
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
                if ui.button("在文件管理器中定位").clicked() {
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

    /// 功能区“导入文档”：选一个现成文档，转成 markdown 插到编辑器光标处。
    ///
    /// 转换本身在主线程同步做——`anydoc` 的量级是毫秒（实测 docx 6ms、xlsx 0.5ms），
    /// 而它前面那个文件选择框本来就要阻塞界面，再为它铺一套后台任务通道不划算。
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
        if !self.doc.warnings.is_empty() {
            self.open_result_drawer();
        }
    }
}
