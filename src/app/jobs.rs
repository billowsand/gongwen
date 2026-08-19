//! 后台任务通道、模型探测与知识库任务。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::jobs`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{ExportOutcome, GongwenApp, KnowledgeImportDraft, KnowledgePreviewState};
use crate::draft_page::{DocKey, DraftSession};
use crate::export;
use crate::knowledge;
use crate::lmstudio;
use crate::manuscript_io;
use crate::models::{DraftInput, GeneratedDraft, RerankMode, ReviewNote};
use crate::pdf_viewer;
use crate::pdf_viewer::PdfKey;
use crate::preview;
use crate::qa;
use crate::rag;
use crate::storage;
use crate::system_fonts;
use crate::theme;
use crate::units::UnitDisplay;
use eframe::egui;
use std::path::PathBuf;
use std::thread;

pub(crate) enum WorkerResult {
    /// 全局任务：探测本地模型服务已加载的模型。
    Models(Result<Vec<String>, String>),
    /// 探测知识库 embedding 端点已加载的模型。
    EmbeddingModels(Result<Vec<String>, String>),
    /// 探测知识库 rerank 端点已加载的模型。
    RerankModels(Result<Vec<String>, String>),
    /// 真跑一次 rerank 的验证结果（端点路径 + 响应字段是否对得上）。
    RerankVerify(Result<String, String>),
    /// 某一篇稿件的任务结果。`key` 认稿件、`seq` 认这一次任务：稿件关了或者
    /// 同一篇又发起了新任务，回来的结果就已作废。
    Doc { key: DocKey, seq: u64, job: DocJob },
    /// 知识库任务：与具体稿件无关的全局任务（索引构建 / 检索测试）。
    Knowledge(KnowledgeJob),
    /// 扫描本机字体目录的结果。中文字体文件很大，扫描放在后台线程。
    SystemFonts(Vec<system_fonts::SystemFont>),
    /// 稿件 PDF 批量导出的结果。`path` 是保存的 zip 路径。
    ManuscriptPdfExport {
        path: PathBuf,
        result: Result<manuscript_io::PdfExportSummary, String>,
    },
    /// PDF 渲染线程的消息：打开完成、某一页光栅化完成或失败。
    /// 纹理在主线程收到后创建。
    Pdf {
        key: PdfKey,
        message: pdf_viewer::PdfMessage,
    },
}

/// 知识库后台任务的结果。
pub(crate) enum KnowledgeJob {
    /// 索引进度（每篇回报一次）。
    IndexProgress {
        done: usize,
        total: usize,
        current_title: String,
    },
    /// 索引整批完成。`failed` 是 (标题, 错误)。
    IndexFinished {
        ok: usize,
        failed: Vec<(String, String)>,
    },
    /// 检索测试的结果。
    SearchDone(Result<rag::RetrievalOutcome, String>),
    /// 知识库问答的结果。`question` 是发起时的原始提问，回来时连同答案一起入历史。
    QaDone {
        question: String,
        result: Result<qa::QaOutcome, String>,
    },
}

/// 知识库页输入区的模式：检索片段列表，还是基于片段生成问答答案。
/// 两种模式共用同一个输入框，切换只改变「发送后干什么」，互不清空各自结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum KnowledgeMode {
    #[default]
    Search,
    Qa,
}

/// 起草页发起的后台任务的结果。
pub(crate) enum DocJob {
    /// 从零起草的结果。
    Drafted(Result<GeneratedDraft, String>),
    Optimized(Result<GeneratedDraft, String>),
    ExportProgress(String),
    Exported(Result<ExportOutcome, String>),
    /// 花脸稿导出结果。与定稿导出分开：花脸稿不是成品，不该顶掉工具栏上
    /// 「打开最近导出」指向的定稿文件。
    RedlineExported(Result<Vec<std::path::PathBuf>, String>),
}

impl GongwenApp {
    pub(crate) fn start_model_probe(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "正在连接本地模型服务并读取已加载模型…".into();
        let config = self.config.lm_studio.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = lmstudio::list_models(&config).map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::Models(result));
        });
    }

    /// 探测知识库 embedding 端点已加载的模型。
    /// 扫描本机字体。中文字体文件动辄十几兆，整轮扫描要一两秒，放后台线程做。
    pub(crate) fn start_system_font_scan(&mut self) {
        if self.system_fonts_busy {
            return;
        }
        self.system_fonts_busy = true;
        self.status = "正在扫描本机字体…".into();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let _ = tx.send(WorkerResult::SystemFonts(system_fonts::scan()));
        });
    }

    pub(crate) fn start_embedding_probe(&mut self) {
        if self.embedding_probe_busy {
            return;
        }
        self.embedding_probe_busy = true;
        self.status = "正在连接 embedding 服务并读取模型…".into();
        let base_url = self.config.rag.embedding.base_url.clone();
        let api_key = self.config.rag.embedding.api_key.clone();
        let timeout = self.config.rag.embedding.timeout_seconds;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = lmstudio::list_models_at(&base_url, &api_key, timeout)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::EmbeddingModels(result));
        });
    }

    /// 探测知识库 rerank 端点已加载的模型。
    pub(crate) fn start_rerank_probe(&mut self) {
        if self.rerank_probe_busy {
            return;
        }
        self.rerank_probe_busy = true;
        self.status = "正在连接 rerank 服务并读取模型…".into();
        let base_url = self.config.rag.rerank.base_url.clone();
        let api_key = self.config.rag.rerank.api_key.clone();
        let timeout = self.config.rag.rerank.timeout_seconds;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = lmstudio::list_models_at(&base_url, &api_key, timeout)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::RerankModels(result));
        });
    }

    /// 真跑一次 rerank，验证端点路径与响应字段是否对得上。
    ///
    /// 只查 `/v1/models` 是不够的：有些服务（如 LM Studio）对不认识的路径会记
    /// `Unexpected endpoint or method` 却仍返回 200，于是"连接成功"，
    /// 而每次检索的 rerank 都在静默失败。
    pub(crate) fn start_rerank_verify(&mut self) {
        if self.rerank_probe_busy {
            return;
        }
        let mode = self.config.rag.rerank.mode;
        match mode {
            RerankMode::None => {
                self.status = "当前未启用重排，无需验证。".into();
                return;
            }
            RerankMode::Api if self.config.rag.rerank.model.trim().is_empty() => {
                self.status = "请先填写或选择 rerank 模型，再验证。".into();
                return;
            }
            RerankMode::Llm if self.config.lm_studio.model.trim().is_empty() => {
                self.status = "请先在上面的「对话模型」里选好模型，再验证。".into();
                return;
            }
            _ => {}
        }
        self.rerank_probe_busy = true;
        self.status = "正在验证重排…".into();
        let cfg = self.config.rag.rerank.clone();
        let chat = self.config.lm_studio.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = match mode {
                RerankMode::Llm => crate::rag_client::probe_rerank_llm(&chat),
                _ => crate::rag_client::probe_rerank(&cfg),
            }
            .map(|n| format!("重排验证通过，返回 {n} 条排序结果。"))
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::RerankVerify(result));
        });
    }

    pub(crate) fn poll_worker(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.receiver.try_recv() {
            match result {
                WorkerResult::Models(Ok(models)) => {
                    self.busy = false;
                    self.models = models;
                    if self.config.lm_studio.model.trim().is_empty() && self.models.len() == 1 {
                        self.config.lm_studio.model = self.models[0].clone();
                    }
                    self.status = if self.models.is_empty() {
                        "已连接，但没有已加载模型。".into()
                    } else {
                        format!("连接成功，发现 {} 个模型。", self.models.len())
                    };
                }
                WorkerResult::Models(Err(error)) => {
                    self.busy = false;
                    self.status = format!("连接失败：{error}");
                }
                WorkerResult::Doc { key, seq, job } => self.apply_doc_job(key, seq, job),
                WorkerResult::Knowledge(job) => self.apply_knowledge_job(job),
                WorkerResult::SystemFonts(fonts) => {
                    self.system_fonts_busy = false;
                    self.system_fonts_scanned = true;
                    self.status = if fonts.is_empty() {
                        "没有在本机字体目录里找到可用的 ttf/otf 字体。".into()
                    } else {
                        format!("已找到 {} 个本机字体。", fonts.len())
                    };
                    self.system_fonts = fonts;
                }
                WorkerResult::EmbeddingModels(result) => {
                    self.embedding_probe_busy = false;
                    match result {
                        Ok(models) => {
                            self.status = if models.is_empty() {
                                "embedding 服务已连接，但没有已加载模型。".into()
                            } else {
                                format!("embedding 服务连接成功，发现 {} 个模型。", models.len())
                            };
                            // 只有一个模型且未配置时自动选上，省去手选。
                            if self.config.rag.embedding.model.trim().is_empty()
                                && models.len() == 1
                            {
                                self.config.rag.embedding.model = models[0].clone();
                            }
                            self.embedding_models = models;
                        }
                        Err(error) => self.status = format!("embedding 连接失败：{error}"),
                    }
                }
                WorkerResult::RerankModels(result) => {
                    self.rerank_probe_busy = false;
                    match result {
                        Ok(models) => {
                            self.status = if models.is_empty() {
                                "rerank 服务已连接，但没有已加载模型。".into()
                            } else {
                                format!("rerank 服务连接成功，发现 {} 个模型。", models.len())
                            };
                            if self.config.rag.rerank.model.trim().is_empty() && models.len() == 1 {
                                self.config.rag.rerank.model = models[0].clone();
                            }
                            self.rerank_models = models;
                        }
                        Err(error) => self.status = format!("rerank 连接失败：{error}"),
                    }
                }
                WorkerResult::RerankVerify(result) => {
                    self.rerank_probe_busy = false;
                    self.rerank_verify_result = Some(match result {
                        Ok(message) => (true, message),
                        Err(error) => (
                            false,
                            match self.config.rag.rerank.mode {
                                RerankMode::Llm => format!(
                                    "用对话大模型重排失败：{error}　可换一个指令跟随更好的对话模型，或把重排方式改为「不重排」。",
                                ),
                                _ => format!(
                                    "rerank 端点验证失败：{error}　请核对「端点路径」——服务对不认识的路径可能照样返回 200，看着像连上了，实际每次重排都会静默失败。LM Studio / Ollama 目前不提供 rerank 专用接口，可改用「用对话大模型重排」。",
                                ),
                            },
                        ),
                    });
                    if let Some((_, message)) = &self.rerank_verify_result {
                        self.status = message.clone();
                    }
                }
                WorkerResult::ManuscriptPdfExport { path, result } => {
                    self.manuscript_pdf_export_busy = false;
                    match result {
                        Ok(summary) => {
                            let mut message = format!(
                                "已导出 {} 篇稿件的 {} 个 PDF 到 {}。",
                                summary.records,
                                summary.pdfs,
                                path.display()
                            );
                            if !summary.failed.is_empty() {
                                let detail = summary
                                    .failed
                                    .iter()
                                    .take(5)
                                    .map(|(title, reason)| format!("{title}：{reason}"))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                let more = summary.failed.len().saturating_sub(5);
                                if more > 0 {
                                    message.push_str(&format!("\n另有 {more} 篇失败原因略。"));
                                }
                                message.push_str(&format!("\n以下稿件未导出：\n{detail}"));
                            }
                            self.status = message;
                        }
                        Err(error) => {
                            self.status =
                                format!("导出 PDF 失败：{error}（未生成 {}）", path.display());
                        }
                    }
                }
                WorkerResult::Pdf { key, message } => {
                    if let Some(index) = self.pdf_index_of_key(key) {
                        self.pdfs[index].apply_message(ctx, message);
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    /// 知识库后台任务的收尾。
    pub(crate) fn apply_knowledge_job(&mut self, job: KnowledgeJob) {
        match job {
            KnowledgeJob::IndexProgress {
                done,
                total,
                current_title,
            } => {
                self.knowledge_index_progress = Some((done, total, current_title));
            }
            KnowledgeJob::IndexFinished { ok, failed } => {
                self.knowledge_busy = false;
                self.knowledge_index_progress = None;
                self.knowledge_dirty = true;
                self.knowledge_index_result = Some(if failed.is_empty() {
                    format!("索引完成：{ok} 篇全部成功。")
                } else {
                    let detail = failed
                        .iter()
                        .take(3)
                        .map(|(title, err)| format!("《{title}》：{err}"))
                        .collect::<Vec<_>>()
                        .join("；");
                    format!(
                        "索引完成：{ok} 篇成功，{} 篇失败（{detail}）。",
                        failed.len()
                    )
                });
            }
            KnowledgeJob::SearchDone(Ok(outcome)) => {
                self.knowledge_busy = false;
                self.knowledge_test_results = outcome.chunks;
                // 降级说明直接摆在检索区，不再只写进服务端日志。
                self.knowledge_search_warnings = outcome.warnings;
            }
            KnowledgeJob::SearchDone(Err(error)) => {
                self.knowledge_busy = false;
                self.knowledge_test_results = Vec::new();
                self.knowledge_search_warnings = Vec::new();
                self.status = format!("知识库检索失败：{error}");
            }
            KnowledgeJob::QaDone { question, result } => {
                self.knowledge_busy = false;
                self.knowledge_qa_pending = None;
                match result {
                    Ok(outcome) => self.knowledge_qa_history.push(qa::QaTurn {
                        question,
                        answer: outcome.answer,
                        references: outcome.references,
                        warnings: outcome.warnings,
                    }),
                    Err(error) => {
                        // 与检索测试同口径：致命错误走全局状态条，不打断页面。
                        self.status = format!("知识库问答失败：{error}");
                    }
                }
            }
        }
    }

    /// 刷新知识库文档列表、块计数、已入库稿件集合与库内嵌入模型。
    pub(crate) fn refresh_knowledge(&mut self) {
        self.knowledge_dirty = false;
        let Some(store) = self.knowledge_store.as_mut() else {
            return;
        };
        match store.list_docs(self.knowledge_filter_kind) {
            Ok(docs) => self.knowledge_docs = docs,
            Err(error) => self.knowledge_error = Some(format!("知识库列表读取失败：{error:#}")),
        }
        self.knowledge_chunk_count = store.count_chunks().unwrap_or(0);
        self.knowledge_indexed_manuscripts = store.indexed_manuscript_ids().unwrap_or_default();
        self.knowledge_embed_models = store.distinct_embed_models().unwrap_or_default();
    }

    /// 库内是否存在与当前配置不同的 embedding 模型。换模型后维度多半不同，
    /// 旧块的余弦恒为 0，会静默退出向量召回——必须提示用户重建索引。
    pub(crate) fn knowledge_embed_model_mismatch(&self) -> Option<String> {
        let current = self.config.rag.embedding.model.trim();
        if current.is_empty() {
            return None;
        }
        let stale: Vec<&str> = self
            .knowledge_embed_models
            .iter()
            .map(String::as_str)
            .filter(|model| *model != current)
            .collect();
        if stale.is_empty() {
            return None;
        }
        Some(format!(
            "库内有文档是用「{}」嵌入的，与当前的「{current}」不一致；向量维度不同会让这些文档静默退出向量检索，请点「重建索引」。",
            stale.join("、")
        ))
    }

    /// 打开外部 markdown 文件选择框，进入导入确认（选文种）。
    pub(crate) fn knowledge_pick_markdown(&mut self) {
        let paths = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown"])
            .pick_files();
        let Some(paths) = paths else { return };
        if paths.is_empty() {
            return;
        }
        self.knowledge_import = Some(KnowledgeImportDraft {
            paths,
            kind: self.config.last_template,
        });
    }

    /// 确认导入外部 markdown：读出文件内容，归一化成导入项后建索引。
    pub(crate) fn knowledge_confirm_import(&mut self) {
        let Some(draft) = self.knowledge_import.take() else {
            return;
        };
        let mut items = Vec::new();
        // 逐个收集读取失败，别让后一条把前一条的错误覆盖掉——选十个文件失败九个
        // 时，只看得到最后一条错误是没法排查的（GBK 编码的 .md 就会走到这里）。
        let mut failures: Vec<String> = Vec::new();
        for path in &draft.paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let title = export::extract_title(&content, "");
                    items.push(knowledge::KnowledgeImportItem {
                        source: knowledge::KnowledgeSource::Markdown,
                        source_manuscript_id: None,
                        source_path: path.display().to_string(),
                        kind: draft.kind,
                        title,
                        content_markdown: content,
                    });
                }
                Err(error) => failures.push(format!("{name}（{error}）")),
            }
        }
        if !failures.is_empty() {
            // 放进 knowledge_error 而不是 status：status 紧接着会被索引进度覆盖。
            self.knowledge_error = Some(format!(
                "{} 个文件读取失败，未加入知识库：{}。若是 GBK 编码，请先另存为 UTF-8。",
                failures.len(),
                failures.join("；")
            ));
        } else {
            self.knowledge_error = None;
        }
        self.start_knowledge_index(items, false);
    }

    /// 把稿件库当前勾选的稿件加入知识库。
    pub(crate) fn knowledge_import_selected_manuscripts(&mut self) {
        let ids: Vec<i64> = self.manuscript_selected.iter().copied().collect();
        if ids.is_empty() {
            self.status = "请先在稿件管理里勾选要加入知识库的稿件。".into();
            return;
        }
        let Some(store) = self.manuscript_store.as_mut() else {
            self.status = "稿件库不可用。".into();
            return;
        };
        let mut items = Vec::new();
        for id in ids {
            match store.get(id) {
                Ok(Some(record)) => items.push(knowledge::KnowledgeImportItem {
                    source: knowledge::KnowledgeSource::Manuscript,
                    source_manuscript_id: Some(record.id),
                    source_path: String::new(),
                    kind: record.kind,
                    title: record.title,
                    content_markdown: record.content_markdown,
                }),
                Ok(None) => {}
                Err(error) => self.status = format!("读取稿件 #{id} 失败：{error:#}"),
            }
        }
        self.start_knowledge_index(items, false);
    }

    /// 重建全部索引：清空后把库内所有文档重新嵌入。这里实现为“对现有文档逐一
    /// 重新切块+嵌入”，用于更换 embedding 模型后。
    pub(crate) fn knowledge_rebuild_all(&mut self) {
        let Some(store) = self.knowledge_store.as_mut() else {
            self.status = "知识库不可用。".into();
            return;
        };
        let docs = match store.list_docs(None) {
            Ok(docs) => docs,
            Err(error) => {
                self.status = format!("读取知识库失败：{error:#}");
                return;
            }
        };
        let mut items = Vec::new();
        for doc in docs {
            if let Ok(Some((title, content))) = store.get_doc_content(doc.id) {
                let source = if doc.source == "manuscript" {
                    knowledge::KnowledgeSource::Manuscript
                } else {
                    knowledge::KnowledgeSource::Markdown
                };
                items.push(knowledge::KnowledgeImportItem {
                    source,
                    source_manuscript_id: doc.source_manuscript_id,
                    source_path: doc.source_path,
                    kind: doc.kind,
                    title,
                    content_markdown: content,
                });
            }
        }
        self.start_knowledge_index(items, true);
    }

    /// 在后台线程跑索引流水线：切块 → 分词 → 批量嵌入 → 入库。
    /// `rebuild` 为 true 时先清空（保留元数据重新嵌入）。
    pub(crate) fn start_knowledge_index(
        &mut self,
        items: Vec<knowledge::KnowledgeImportItem>,
        rebuild: bool,
    ) {
        if self.knowledge_busy {
            self.status = "知识库任务正在进行中…".into();
            return;
        }
        if items.is_empty() {
            if !rebuild {
                self.status = "没有可索引的内容。".into();
            }
            return;
        }
        if self.config.rag.embedding.model.trim().is_empty() {
            self.status = "请先在“设置”中配置知识库 embedding 模型。".into();
            return;
        }
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            self.status = "知识库路径不可用。".into();
            return;
        };
        self.knowledge_busy = true;
        self.knowledge_index_result = None;
        self.knowledge_index_progress = Some((0, items.len(), String::new()));
        let cfg = self.config.rag.clone();
        let tx = self.sender.clone();
        let _ = rebuild; // replace_document 已按来源去重，重建等价于逐篇重嵌。
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let (ok, failed) = knowledge::run_index_pipeline_with(
                db_path,
                cfg,
                items,
                move |done, total, title| {
                    let _ =
                        progress_tx.send(WorkerResult::Knowledge(KnowledgeJob::IndexProgress {
                            done,
                            total,
                            current_title: title,
                        }));
                },
            );
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::IndexFinished {
                ok,
                failed,
            }));
        });
    }

    /// 在后台线程跑一次检索测试。
    pub(crate) fn knowledge_test_search(&mut self) {
        if self.knowledge_busy {
            return;
        }
        let query = self.knowledge_test_query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            return;
        };
        self.knowledge_busy = true;
        self.knowledge_search_warnings.clear();
        let cfg = self.config.rag.clone();
        // 重排走「对话大模型」模式时要用到聊天模型的配置。
        let chat = self.config.lm_studio.clone();
        let kind = self.knowledge_filter_kind;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = rag::retrieve(&cfg, &chat, &db_path, &query, kind)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::SearchDone(result)));
        });
    }

    /// 在后台线程跑一次知识库问答：检索片段 → 生成答案。问题带回时连同答案
    /// 一起入历史，等待期间 `knowledge_qa_pending` 记录原问题供对话区展示。
    pub(crate) fn knowledge_ask(&mut self) {
        if self.knowledge_busy {
            return;
        }
        let question = self.knowledge_test_query.trim().to_string();
        if question.is_empty() {
            return;
        }
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            return;
        };
        self.knowledge_busy = true;
        self.knowledge_qa_pending = Some(question.clone());
        let cfg = self.config.rag.clone();
        // 问答要调对话模型生成答案，重排若走「对话大模型」模式也需要聊天配置。
        let chat = self.config.lm_studio.clone();
        let kind = self.knowledge_filter_kind;
        let history = self.knowledge_qa_history.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = qa::qa_answer(&cfg, &chat, &db_path, &history, &question, kind)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::QaDone {
                question,
                result,
            }));
        });
    }

    /// 删除一篇知识库文档。
    pub(crate) fn knowledge_delete(&mut self, id: i64) {
        if let Some(store) = self.knowledge_store.as_mut() {
            match store.delete_document(id) {
                Ok(()) => {
                    self.status = "已从知识库删除。".into();
                    self.knowledge_dirty = true;
                }
                Err(error) => self.status = format!("删除失败：{error:#}"),
            }
        }
        self.knowledge_delete_confirm = None;
    }

    /// 打开知识库文档预览弹窗：从库里读出标题、文种、原文。
    pub(crate) fn knowledge_open_preview(&mut self, doc_id: i64) {
        let Some(store) = self.knowledge_store.as_mut() else {
            self.status = "知识库不可用。".into();
            return;
        };
        match store.get_doc_for_preview(doc_id) {
            Ok(Some((title, kind, markdown))) => {
                self.knowledge_preview = Some(KnowledgePreviewState {
                    title,
                    kind,
                    markdown,
                    zoom: None,
                    fit_scale: 1.0,
                });
            }
            Ok(None) => self.status = "该文档已不在知识库中。".into(),
            Err(error) => self.status = format!("读取知识库文档失败：{error:#}"),
        }
    }

    /// 知识库文档预览浮窗：复用起草页的公文版式渲染，按窗口宽度自适应缩放。
    pub(crate) fn knowledge_preview_window(&mut self, ctx: &egui::Context) {
        let Some(preview) = self.knowledge_preview.as_mut() else {
            theme::reset_window_anim(ctx, egui::Id::new("knowledge_preview_anim"));
            return;
        };
        let mut open = true;
        let mut close_clicked = false;
        let win = egui::Window::new(format!("预览 · {}", preview.title))
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 720.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                // 顶部一行：缩放控件 + 关闭按钮。
                ui.horizontal(|ui| {
                    let current = preview.zoom.unwrap_or(preview.fit_scale);
                    if theme::icon_button(ui, theme::Icon::ZoomIn, "放大").clicked() {
                        preview.zoom = Some((current + 0.1).min(2.0));
                    }
                    ui.label(
                        egui::RichText::new(format!("{:.0}%", current * 100.0))
                            .color(theme::text_muted()),
                    );
                    if theme::icon_button(ui, theme::Icon::ZoomOut, "缩小").clicked() {
                        preview.zoom = Some((current - 0.1).max(0.4));
                    }
                    if theme::icon_button_enabled(
                        ui,
                        preview.zoom.is_some(),
                        theme::Icon::FitWidth,
                        "适应宽度",
                    )
                    .clicked()
                    {
                        preview.zoom = None;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("关闭").clicked() {
                            close_clicked = true;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::both()
                    .id_salt("knowledge_preview_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let display = UnitDisplay::new(&self.config.vocabulary);
                        // 知识库文档只存了文种与原文；版式要素用该文种的默认配置补全。
                        let mut input = DraftInput {
                            kind: preview.kind,
                            ..Default::default()
                        };
                        input.profile = self.config.profile(preview.kind);
                        let output = preview::official_preview(
                            ui,
                            &input,
                            &display,
                            &preview.markdown,
                            preview::PreviewScale::zoom(preview.zoom),
                            None,
                            false,
                        );
                        preview.fit_scale = output.scale;
                    });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("knowledge_preview_anim"), &w.response);
        }
        if !open || close_clicked {
            self.knowledge_preview = None;
        }
    }

    /// 生成与优化的收尾一致：换正文、换审校结果，有提示就弹审校抽屉。
    pub(crate) fn take_generated(doc: &mut DraftSession, result: GeneratedDraft) {
        doc.proof_markdown = if result.proof_measured {
            result.markdown.clone()
        } else {
            String::new()
        };
        doc.proof_warnings = result.proof_warnings;
        doc.generated_markdown = result.markdown;
        doc.warnings = result.warnings;
        doc.warnings.extend(doc.proof_warnings.iter().cloned());
        doc.output_files = result.files;
        doc.export_error = None;
        if !doc.warnings.is_empty() {
            doc.result_drawer_open = true;
        }
    }

    /// 后台任务回投。稿件可能已经关闭，或者同一篇又发起了新任务——
    /// 两种情况下这份结果都已作废，直接丢掉，绝不能落到别的稿件上。
    pub(crate) fn apply_doc_job(&mut self, key: DocKey, seq: u64, job: DocJob) {
        let Some(index) = self.docs.iter().position(|doc| doc.key == key) else {
            return;
        };
        if self.docs[index].job_seq != seq {
            return;
        }
        if !matches!(job, DocJob::ExportProgress(_)) {
            self.docs[index].busy = false;
        }
        // 后台跑完的未必是当前显示的那篇，状态栏要点名是谁。
        let prefix = if index == self.active_doc {
            String::new()
        } else {
            format!("《{}》", self.docs[index].title())
        };
        match job {
            DocJob::Drafted(Ok(result)) => {
                let title = result.title.clone();
                Self::take_generated(&mut self.docs[index], result);
                self.status = if self.docs[index].output_files.is_empty() {
                    format!(
                        "{prefix}“{title}”草稿已生成。可直接在右侧修改，然后点“导出当前审校稿”。"
                    )
                } else {
                    format!(
                        "{prefix}“{title}”已生成并导出 {} 个文件。",
                        self.docs[index].output_files.len()
                    )
                };
            }
            DocJob::Optimized(Ok(result)) => {
                let title = result.title.clone();
                Self::take_generated(&mut self.docs[index], result);
                self.status = if self.docs[index].output_files.is_empty() {
                    format!(
                        "{prefix}“{title}”已按“{}”优化，输出格式已按内置标准校正。",
                        self.docs[index].ai_prompt_last_label
                    )
                } else {
                    format!(
                        "{prefix}“{title}”已按“{}”优化并导出 {} 个文件。",
                        self.docs[index].ai_prompt_last_label,
                        self.docs[index].output_files.len()
                    )
                };
            }
            DocJob::Drafted(Err(error)) => self.status = format!("{prefix}起草失败：{error}"),
            DocJob::Optimized(Err(error)) => self.status = format!("{prefix}优化失败：{error}"),
            DocJob::ExportProgress(message) => self.status = format!("{prefix}{message}"),
            DocJob::Exported(Ok(outcome)) => {
                self.docs[index].output_files = outcome.files;
                // 编译失败也走 export_error：审校抽屉顶部的红色框会高亮显示，区别于
                // 审校提示里的样式警告。md/docx/tex 仍然成功导出，只缺 PDF。
                self.docs[index].export_error = outcome.compile_error;
                // 先落孤行实测结果，再 revalidate——它会把这批提示并进审校列表。
                if outcome.proof_measured {
                    self.docs[index].proof_markdown = self.docs[index].generated_markdown.clone();
                    self.docs[index].proof_warnings = outcome.proof_warnings;
                }
                self.draft_page_at(index).revalidate();
                self.docs[index]
                    .warnings
                    .extend(outcome.warnings.into_iter().map(ReviewNote::from));
                self.docs[index]
                    .warnings
                    .sort_by(|a, b| a.message.cmp(&b.message));
                self.docs[index]
                    .warnings
                    .dedup_by(|a, b| a.message == b.message);
                // 成品不再进抽屉：让工具栏那三枚 TEX/PDF/WORD 入口重新扫盘点亮即可。
                self.export_links.invalidate();
                let orphans = self.docs[index].proof_warnings.len();
                // 孤行或编译失败都是导出这一下才能看到的，弹抽屉把它们顶到用户眼前，
                // 别让红色错误提示躺在折叠面板里看不见。
                if orphans > 0 || self.docs[index].export_error.is_some() {
                    self.docs[index].result_drawer_open = true;
                }
                self.status = if self.docs[index].export_error.is_some() {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件；PDF 编译失败，请查看审校提示。",
                        self.docs[index].output_files.len()
                    )
                } else if orphans > 0 {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件；实测发现 {orphans} 处孤行，见审校提示。",
                        self.docs[index].output_files.len()
                    )
                } else {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件。",
                        self.docs[index].output_files.len()
                    )
                };
            }
            DocJob::RedlineExported(Ok(files)) => {
                let pdf = files
                    .iter()
                    .find(|file| file.extension().is_some_and(|ext| ext == "pdf"))
                    .cloned();
                self.status = match files.first() {
                    Some(first) => format!(
                        "{prefix}花脸稿已导出到 {}。",
                        first.parent().unwrap_or(first).display()
                    ),
                    None => format!("{prefix}花脸稿没有产生任何文件。"),
                };
                // 出了 PDF 就直接在应用内打开，省得再去翻目录。
                if let Some(pdf) = pdf {
                    self.open_pdf(pdf, Some("花脸稿".to_string()));
                }
            }
            DocJob::RedlineExported(Err(error)) => {
                self.status = format!("{prefix}花脸稿导出失败：{error}");
            }
            DocJob::Exported(Err(error)) => {
                self.status = format!("{prefix}导出失败：{error}");
                let doc = &mut self.docs[index];
                doc.export_error = Some(error);
                // 失败原因全文挂在审校抽屉顶上，弹出来让人看见。
                doc.result_drawer_open = true;
            }
        }
    }
}
