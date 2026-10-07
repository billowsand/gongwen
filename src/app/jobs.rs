//! 后台任务通道、模型探测与知识库任务。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::jobs`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::ai_panel::TurnState;
use crate::app::{ExportOutcome, GongwenApp, KnowledgeImportDraft, KnowledgePreviewState};
use crate::doc_import;
use crate::draft_page::{DocKey, DraftSession};
use crate::export;
use crate::knowledge;
use crate::lmstudio;
use crate::manuscript_io;
use crate::models::{DraftInput, GeneratedDraft, ModelKind, RerankMode, ReviewNote};
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
    /// 探测一家提供商：连接结果与已加载模型清单。
    /// `probed_kinds` 是服务原生接口自报的模型用途（目前只有 LM Studio 给），
    /// 写回配置前还要过一遍 `classify_model` 补名字启发式。
    ProviderModels {
        provider_id: String,
        result: Result<Vec<String>, String>,
        probed_kinds: std::collections::HashMap<String, ModelKind>,
    },
    /// 真跑一次 rerank 的验证结果（端点路径 + 响应字段是否对得上）。
    RerankVerify(Result<String, String>),
    /// 某一篇稿件的任务结果。`key` 认稿件、`seq` 认这一次任务：稿件关了或者
    /// 同一篇又发起了新任务，回来的结果就已作废。
    Doc { key: DocKey, seq: u64, job: DocJob },
    /// 版本对照右栏的花脸稿在后台算好了。不走 `Doc` 的任务序号：它不占用
    /// 「忙」状态，也不该被同一篇稿件的其他任务作废；是否过期按稿件、基准版与
    /// 正文哈希三者判断——只看正文哈希的话，任务在途时换了基准版，晚到的旧基准
    /// 结果会被当成新的收下。
    Redline {
        key: DocKey,
        base: (i64, i64),
        hash: u64,
        // 装箱：RedlineDoc 带要素标注后比其他变体大不少，枚举就地传递不划算。
        doc: Box<crate::redline::RedlineDoc>,
    },
    /// 知识库任务：与具体稿件无关的全局任务（索引构建 / 检索测试）。
    Knowledge(KnowledgeJob),
    /// 公文词表任务：语料扫描的进度与结果。
    Lexicon(crate::app::LexiconJob),
    /// 扫描本机字体目录的结果。中文字体文件很大，扫描放在后台线程。
    SystemFonts(Vec<system_fonts::SystemFont>),
    /// 送批材料合并导出的进度与结果。
    SendPackage(crate::app::SendPackageEvent),
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
    /// PDF 打印任务（后台线程跑系统打印对话框与逐页光栅化）的结果。
    PdfPrinted {
        path: PathBuf,
        result: Result<crate::print_pdf::PrintOutcome, String>,
    },
}

/// 知识库后台任务的结果。
pub(crate) enum KnowledgeJob {
    /// 导入或索引的进度（每篇回报一次）。
    IndexProgress {
        done: usize,
        total: usize,
        current_title: String,
    },
    /// 外部文档导入整批完成（只入库、未索引）。`failed` 是 (文件名, 错误)。
    ImportFinished {
        ok: usize,
        failed: Vec<(String, String)>,
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
    /// 模型的流式增量，落进 AI 侧栏正在跑的那一轮，不碰正文。`done` 表示模型
    /// 已经说完、接下来是程序校验。
    AiStream {
        content: String,
        reasoning: String,
        done: bool,
    },
    /// 要留在 AI 侧栏卡片上的说明（知识库命中了哪几篇、为什么没命中）。
    AiNote(String),
    /// 技能流程的一次工具调用，侧栏「过程」里一行。
    AiTool(String),
    AiUsage {
        turn_id: u64,
        usage: crate::agent::backend::UsageTotals,
    },
    /// 技能流程的工作稿整体换新（补全之后）。
    AiWorkspace(String),
    AiWriteBegin {
        prefix: String,
        suffix: String,
    },
    /// 技能任务结束：流程挂起要问的题，或定稿后的提案。
    SkillDone(Result<Box<crate::ai_panel::SkillResult>, String>),
    /// 技能任务开跑前压缩了会话：新的会话摘要与它覆盖到的最后一轮。
    AiSessionSummary {
        summary: String,
        upto: u64,
    },
    /// 手动压缩会话的结果：(摘要, 覆盖到的最后一轮, 并进去几轮)。
    AiCompactDone(Result<(String, u64, usize), String>),
    /// 这一轮用了哪份风格档案（记一次使用）。
    StyleUsed(String),
    /// 按回答修订的结果：AI 把缺口题的回答写进了所在段落（或闸门不过、直接替换）。
    GapRevised(Box<crate::ai_panel::GapRevision>),
    ExportProgress(String),
    Exported(Result<ExportOutcome, String>),
    /// 花脸稿导出结果。与定稿导出分开：花脸稿不是成品，不该顶掉工具栏上
    /// 「打开最近导出」指向的定稿文件。
    RedlineExported(Result<Vec<std::path::PathBuf>, String>),
    /// 花脸稿打印预览：只出 PDF、落在临时目录，编完在内置查看器里打开。
    RedlinePrintPreview(Result<Vec<std::path::PathBuf>, String>),
    /// 小模型逐句文字复核的结果。只产出待确认的建议，不碰正文。
    Reviewed(Result<crate::revise_model::ReviewOutcome, String>),
}

impl GongwenApp {
    /// 探测一家提供商：测试连接并把已加载模型清单缓存进配置。
    ///
    /// 各家独立进行，互不阻塞；状态挂在提供商卡片上（`provider_status`）。
    pub(crate) fn start_provider_probe(&mut self, provider_id: &str) {
        let Some(provider) = self.config.provider(provider_id) else {
            return;
        };
        let status = self
            .provider_status
            .entry(provider_id.to_string())
            .or_default();
        if status.busy {
            return;
        }
        status.busy = true;
        self.status = format!("正在连接「{}」并读取已加载模型…", provider.name);
        let base_url = provider.base_url.clone();
        let api_key = provider.api_key.clone();
        let timeout = self.config.lm_studio.timeout_seconds;
        let pid = provider_id.to_string();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = lmstudio::list_models_at(&base_url, &api_key, timeout)
                .map_err(|e| format!("{e:#}"));
            // 顺带问原生接口要模型用途；不是 LM Studio 就空表，靠名字归类。
            let probed_kinds = if result.is_ok() {
                lmstudio::native_model_kinds(&base_url, &api_key, timeout).unwrap_or_default()
            } else {
                std::collections::HashMap::new()
            };
            let _ = tx.send(WorkerResult::ProviderModels {
                provider_id: pid,
                result,
                probed_kinds,
            });
        });
    }

    /// 刷新所有已启用提供商的模型清单。
    pub(crate) fn start_all_provider_probes(&mut self) {
        let ids: Vec<String> = self
            .config
            .providers
            .iter()
            .filter(|p| p.enabled && !p.base_url.trim().is_empty())
            .map(|p| p.id.clone())
            .collect();
        for id in ids {
            self.start_provider_probe(&id);
        }
    }

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

    /// 真跑一次 rerank，验证端点路径与响应字段是否对得上。
    ///
    /// 只查 `/v1/models` 是不够的：有些服务（如 LM Studio）对不认识的路径会记
    /// `Unexpected endpoint or method` 却仍返回 200，于是"连接成功"，
    /// 而每次检索的 rerank 都在静默失败。
    pub(crate) fn start_rerank_verify(&mut self) {
        if self.rerank_verify_busy {
            return;
        }
        let mode = self.config.rag.rerank.mode;
        match mode {
            RerankMode::None => {
                self.status = "当前未启用重排，无需验证。".into();
                return;
            }
            RerankMode::Api if self.config.rag.effective_rerank_mode() == RerankMode::None => {
                self.status = "请先选择 rerank 模型，再验证。".into();
                return;
            }
            _ => {}
        }
        self.rerank_verify_busy = true;
        self.status = "正在验证重排…".into();
        let cfg = self.config.resolved_rag().rerank;
        let chat = match self.config.draft_chat() {
            Ok(chat) => chat,
            Err(error) => {
                self.rerank_verify_busy = false;
                self.status = error.to_string();
                return;
            }
        };
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
                WorkerResult::ProviderModels {
                    provider_id,
                    result,
                    probed_kinds,
                } => {
                    let status = self.provider_status.entry(provider_id.clone()).or_default();
                    status.busy = false;
                    let name = self
                        .config
                        .provider(&provider_id)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| provider_id.clone());
                    match result {
                        Ok(models) => {
                            status.connected = true;
                            status.note = if models.is_empty() {
                                "已连接，但没有已加载模型。".into()
                            } else {
                                format!("连接成功，发现 {} 个模型。", models.len())
                            };
                            // 只有一个模型且起草还没选时自动选上，省去手选。
                            if self.config.draft_model.is_empty() && models.len() == 1 {
                                self.config.draft_model = crate::models::ModelRef {
                                    provider_id: provider_id.clone(),
                                    model: models[0].clone(),
                                };
                            }
                            if let Some(provider) = self.config.provider_mut(&provider_id) {
                                // 用途归类：服务自报优先，其余按名字，认不出留 Unknown。
                                provider.kinds = models
                                    .iter()
                                    .map(|model| {
                                        (
                                            model.clone(),
                                            crate::models::classify_model(
                                                model,
                                                probed_kinds.get(model).copied(),
                                            ),
                                        )
                                    })
                                    .collect();
                                provider.models = models;
                            }
                            self.status =
                                format!("「{name}」{}", self.provider_status[&provider_id].note);
                        }
                        Err(error) => {
                            status.connected = false;
                            status.note = format!("连接失败：{error}");
                            self.status = format!("「{name}」连接失败：{error}");
                        }
                    }
                }
                WorkerResult::Doc { key, seq, job } => self.apply_doc_job(key, seq, job),
                WorkerResult::Redline {
                    key,
                    base,
                    hash,
                    doc,
                } => {
                    if let Some(session) = self.docs.iter_mut().find(|session| session.key == key) {
                        session.draft_diff.accept_redline(base, hash, *doc);
                    }
                }
                WorkerResult::Knowledge(job) => self.apply_knowledge_job(job),
                WorkerResult::Lexicon(job) => self.handle_lexicon_job(job),
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
                WorkerResult::RerankVerify(result) => {
                    self.rerank_verify_busy = false;
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
                WorkerResult::SendPackage(event) => self.handle_send_package_event(event),
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
                WorkerResult::PdfPrinted { path, result } => {
                    self.status = match result {
                        Ok(crate::print_pdf::PrintOutcome::Printed) => {
                            format!("已送打印机：{}。", path.display())
                        }
                        Ok(crate::print_pdf::PrintOutcome::Cancelled) => "已取消打印。".into(),
                        Err(error) => format!("打印失败：{error}"),
                    };
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
            KnowledgeJob::ImportFinished { ok, failed } => {
                self.knowledge_busy = false;
                self.knowledge_index_progress = None;
                self.knowledge_dirty = true;
                let next = if ok == 0 {
                    String::new()
                } else {
                    "，尚未建立索引，点「建立索引」后才能被检索".to_string()
                };
                self.knowledge_index_result = Some(format!(
                    "导入完成：{ok} 篇成功{next}{}。",
                    failure_summary(&failed)
                ));
            }
            KnowledgeJob::IndexFinished { ok, failed } => {
                self.knowledge_busy = false;
                self.knowledge_index_progress = None;
                self.knowledge_dirty = true;
                self.knowledge_index_result = Some(if failed.is_empty() {
                    format!("索引完成：{ok} 篇全部成功。")
                } else {
                    format!("索引完成：{ok} 篇成功{}。", failure_summary(&failed))
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
        self.knowledge_unindexed = store.unindexed_doc_ids().map_or(0, |ids| ids.len());
    }

    /// 库内是否存在与当前配置不同的 embedding 模型。换模型后维度多半不同，
    /// 旧块的余弦恒为 0，会静默退出向量召回——必须提示用户重建索引。
    pub(crate) fn knowledge_embed_model_mismatch(&self) -> Option<String> {
        let current = self.config.embedding_model_name();
        let current = current.trim();
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

    /// 打开外部文档选择框（Markdown / Word / 电子版 PDF 等），进入导入确认（选文种）。
    pub(crate) fn knowledge_pick_documents(&mut self) {
        let Some(paths) = doc_import::pick_knowledge_files() else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        self.knowledge_import = Some(KnowledgeImportDraft {
            paths,
            kind: self.config.last_template,
        });
    }

    /// 确认导入外部文档：在后台线程逐个转成 markdown 存进知识库，不建索引。
    ///
    /// 导入与索引分开：导入只需本地转换，不依赖 embedding 服务，导完立刻在「库内
    /// 文档」里看得到；索引另点「建立索引」。转换放进后台：单个 docx 是毫秒级，
    /// 但一次选几十个文件、或几百页的 PDF 就会卡住界面。失败（扫描版 PDF、加密、
    /// 格式坏了）逐个记下，汇总在结果摘要里，不让后一条把前一条覆盖掉。
    pub(crate) fn knowledge_confirm_import(&mut self) {
        let Some(draft) = self.knowledge_import.take() else {
            return;
        };
        if self.knowledge_busy {
            self.knowledge_error = Some("知识库任务正在进行中，请稍候再导入。".into());
            return;
        }
        let db_path = match storage::manuscript_db_path() {
            Ok(path) => path,
            Err(error) => {
                self.knowledge_error = Some(format!("知识库路径不可用：{error:#}"));
                return;
            }
        };
        self.knowledge_error = None;
        let kind = draft.kind;
        let paths = draft.paths;
        let total = paths.len();
        self.knowledge_busy = true;
        self.knowledge_index_result = None;
        self.knowledge_progress_verb = "正在导入";
        self.knowledge_index_progress = Some((0, total, String::new()));
        let tx = self.sender.clone();
        thread::spawn(move || {
            let mut ok = 0usize;
            let mut failed: Vec<(String, String)> = Vec::new();
            let mut store = match knowledge::KnowledgeStore::open(&db_path) {
                Ok(store) => store,
                Err(error) => {
                    failed.push(("知识库".into(), format!("打开知识库失败：{error:#}")));
                    let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::ImportFinished {
                        ok,
                        failed,
                    }));
                    return;
                }
            };
            for (index, path) in paths.iter().enumerate() {
                let name = doc_import::file_label(path);
                let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::IndexProgress {
                    done: index,
                    total,
                    current_title: name.clone(),
                }));
                let content = match doc_import::to_knowledge_markdown(path) {
                    Ok(content) => content,
                    // 摘要里已经有文件名，只留根因，别把"解析 X 失败"再说一遍。
                    Err(error) => {
                        failed.push((name, error.root_cause().to_string()));
                        continue;
                    }
                };
                // Word / PDF 转出来的大多没有 `# ` 大标题，用文件名兜底，
                // 免得整库都叫「未命名公文」。
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_default();
                let item = knowledge::KnowledgeImportItem {
                    source: knowledge::KnowledgeSource::Markdown,
                    source_manuscript_id: None,
                    source_path: path.display().to_string(),
                    kind,
                    title: export::extract_title(&content, &stem),
                    content_markdown: content,
                };
                match store.import_document(&item) {
                    Ok(_) => ok += 1,
                    Err(error) => failed.push((name, format!("写入知识库失败：{error:#}"))),
                }
            }
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::ImportFinished {
                ok,
                failed,
            }));
        });
    }

    /// 把稿件库当前勾选的稿件加入知识库（只入库，不建索引）。稿件正文已在库里，
    /// 写一遍是毫秒级，直接在主线程做。
    pub(crate) fn knowledge_import_selected_manuscripts(&mut self) {
        let ids: Vec<i64> = self.manuscript_selected.iter().copied().collect();
        if ids.is_empty() {
            self.status = "请先在稿件管理里勾选要加入知识库的稿件。".into();
            return;
        }
        let Some(manuscripts) = self.manuscript_store.as_mut() else {
            self.status = "稿件库不可用。".into();
            return;
        };
        let mut items = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        for id in ids {
            match manuscripts.get(id) {
                Ok(Some(record)) => items.push(knowledge::KnowledgeImportItem {
                    source: knowledge::KnowledgeSource::Manuscript,
                    source_manuscript_id: Some(record.id),
                    source_path: String::new(),
                    kind: record.kind,
                    title: record.title,
                    content_markdown: record.content_markdown,
                }),
                Ok(None) => {}
                Err(error) => failed.push((format!("稿件 #{id}"), format!("{error:#}"))),
            }
        }
        let Some(store) = self.knowledge_store.as_mut() else {
            self.status = "知识库不可用。".into();
            return;
        };
        let mut ok = 0usize;
        for item in &items {
            match store.import_document(item) {
                Ok(_) => ok += 1,
                Err(error) => failed.push((item.title.clone(), format!("{error:#}"))),
            }
        }
        self.knowledge_dirty = true;
        let message = format!(
            "已把 {ok} 篇稿件加入知识库{}{}。",
            if ok > 0 {
                "，到知识库页点「建立索引」后才能被检索"
            } else {
                ""
            },
            failure_summary(&failed)
        );
        self.status = message.clone();
        self.knowledge_index_result = Some(message);
    }

    /// 为已导入、尚未索引的文档建立索引。
    pub(crate) fn knowledge_build_index(&mut self) {
        let ids = match self.knowledge_store.as_mut().map(|s| s.unindexed_doc_ids()) {
            Some(Ok(ids)) => ids,
            Some(Err(error)) => {
                self.knowledge_error = Some(format!("读取知识库失败：{error:#}"));
                return;
            }
            None => {
                self.knowledge_error = Some("知识库不可用。".into());
                return;
            }
        };
        if ids.is_empty() {
            self.knowledge_index_result = Some("没有待索引的文档。".into());
            return;
        }
        self.start_knowledge_index(ids);
    }

    /// 重建全部索引：对库内所有文档逐一重新切块+嵌入，用于更换 embedding 模型后。
    pub(crate) fn knowledge_rebuild_all(&mut self) {
        let ids = match self.knowledge_store.as_mut().map(|s| s.all_doc_ids()) {
            Some(Ok(ids)) => ids,
            Some(Err(error)) => {
                self.knowledge_error = Some(format!("读取知识库失败：{error:#}"));
                return;
            }
            None => {
                self.knowledge_error = Some("知识库不可用。".into());
                return;
            }
        };
        if ids.is_empty() {
            self.knowledge_index_result = Some("知识库还是空的，先导入文档。".into());
            return;
        }
        self.start_knowledge_index(ids);
    }

    /// 在后台线程跑索引流水线：切块 → 分词 → 批量嵌入 → 原地替换切块。
    ///
    /// 前置条件不满足时的提示放进 `knowledge_error`：它就挂在知识库页上，
    /// 而 `status` 在这一页看不到，点了按钮没反应会让人以为程序坏了。
    fn start_knowledge_index(&mut self, doc_ids: Vec<i64>) {
        if self.knowledge_busy {
            self.knowledge_error = Some("知识库任务正在进行中，请稍候。".into());
            return;
        }
        if self.config.embedding_model_name().is_empty() {
            self.knowledge_error = Some(
                "还没有配置 embedding 模型，无法建立索引：请先到「AI 管理 → 模型服务」的知识库检索块选择。文档已在库内，配置好后再点「建立索引」即可。"
                    .into(),
            );
            return;
        }
        let db_path = match storage::manuscript_db_path() {
            Ok(path) => path,
            Err(error) => {
                self.knowledge_error = Some(format!("知识库路径不可用：{error:#}"));
                return;
            }
        };
        self.knowledge_error = None;
        self.knowledge_busy = true;
        self.knowledge_index_result = None;
        self.knowledge_progress_verb = "正在建立索引";
        self.knowledge_index_progress = Some((0, doc_ids.len(), String::new()));
        let cfg = self.config.resolved_rag();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let (ok, failed) = knowledge::run_index_pipeline_with(
                db_path,
                cfg,
                doc_ids,
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
        let cfg = self.config.resolved_rag();
        // 重排走「对话大模型」模式时要用到聊天模型的配置。
        let chat = self.config.draft_chat().unwrap_or_default();
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
        let cfg = self.config.resolved_rag();
        // 问答要调对话模型生成答案，重排若走「对话大模型」模式也需要聊天配置。
        let chat = self.config.draft_chat().unwrap_or_default();
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
                            &self.config.numbering,
                            // 知识库里的文档没有版本对照，不画要素标注。
                            false,
                            &crate::visual_diff::ElementMarks::default(),
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

    /// 研究式起草结束。
    fn apply_skill_done(
        &mut self,
        index: usize,
        prefix: &str,
        result: Result<Box<crate::ai_panel::SkillResult>, String>,
    ) {
        use crate::ai_panel::{ResearchSnapshot, SkillResult};
        let doc = &mut self.docs[index];
        match result.map(|boxed| *boxed) {
            Ok(SkillResult::Suspended(run)) => {
                doc.ai_review_baseline = None;
                let count = run.suspension.questions.len();
                doc.ai_panel.ask(run);
                self.status = format!("{prefix}有 {count} 个问题要你确认，在侧栏里选一下。");
            }
            Ok(SkillResult::Report {
                skill,
                skill_id,
                findings,
                style,
            }) => {
                doc.ai_review_baseline = None;
                let suggestions: Vec<crate::revision::ModelSuggestion> = findings
                    .iter()
                    .filter_map(|finding| {
                        let fix = finding.fix.as_ref()?;
                        Some(crate::revision::ModelSuggestion {
                            span: fix.span.clone(),
                            before: fix.before.clone(),
                            after: fix.after.clone(),
                            reason: finding.text.clone(),
                            task: format!("skill:{skill_id}"),
                            group: finding.group.clone(),
                        })
                    })
                    .collect();
                let offered = suggestions.len();
                let markdown = std::mem::take(&mut doc.generated_markdown);
                let dropped = doc.revisions.merge_model(
                    &format!("skill:{skill_id}"),
                    suggestions,
                    &markdown,
                    &self.config.proofread.ignored,
                );
                doc.generated_markdown = markdown;
                let fixes = offered - dropped;
                let count = findings.len();
                if let Some(turn) = doc.ai_panel.running_turn_mut() {
                    turn.findings = findings;
                    turn.style = style.map(|profile| *profile);
                }
                doc.ai_panel.finish(TurnState::Reported { fixes });
                if fixes > 0 {
                    doc.result_drawer_open = true;
                }
                self.status = match (count, fixes) {
                    (0, _) => format!("{prefix}{skill}完成：没有发现问题。"),
                    (_, 0) => format!("{prefix}{skill}完成：{count} 条，清单在侧栏里。"),
                    _ => format!(
                        "{prefix}{skill}完成：{count} 条，其中 {fixes} 条有改法，已放进审校抽屉逐条采纳。"
                    ),
                };
            }
            Ok(SkillResult::Proposal {
                skill,
                draft,
                report,
            }) => {
                let report = *report;
                let before = doc
                    .ai_review_baseline
                    .take()
                    .unwrap_or_else(|| doc.generated_markdown.clone());
                let turn_id = doc.ai_panel.running_turn_mut().map(|turn| {
                    // 自动选技能时卡片抬头先写着「自动选择技能」，定下来后换成技能名。
                    if turn.title.starts_with("自动选择技能") {
                        turn.title = turn.title.replacen("自动选择技能", &skill, 1);
                    }
                    turn.id
                });
                if doc.ai_prompt_last_label.starts_with("自动选择技能") {
                    doc.ai_prompt_last_label =
                        doc.ai_prompt_last_label.replacen("自动选择技能", &skill, 1);
                }
                let label = doc.ai_prompt_last_label.clone();
                let title = draft.title.clone();
                let summary =
                    Self::install_ai_proposal(doc, before, draft, label, &self.config.vocabulary);
                doc.ai_panel.finish(TurnState::Proposed(summary));
                // 新提案到了：下一轮默认接着改它（「改提案」）。
                doc.ai_panel.composer.skip_proposal = false;
                let (resolved, _, pending) = report.ledger.counts();
                let asked = report.questions.len();
                let rounds = report.rounds;
                let researched = !report.ledger.gaps.is_empty() || !report.evidence.is_empty();
                if let Some(turn) = turn_id.and_then(|id| doc.ai_panel.turn_mut(id)) {
                    turn.replies = crate::ai_panel::initial_replies(&report.questions);
                    turn.questions = report.questions;
                    if researched {
                        turn.research = Some(ResearchSnapshot {
                            raw: report.markdown,
                            sources: report
                                .evidence
                                .items()
                                .iter()
                                .map(|item| (item.id, item.source_label()))
                                .collect(),
                            ledger: report.ledger,
                        });
                    }
                }
                self.status = if !researched {
                    format!("{prefix}{skill}完成：《{title}》的提案在左侧 AI 工作稿里。")
                } else if asked == 0 {
                    format!(
                        "{prefix}{skill}完成（{rounds} 轮）：补全 {resolved} 处缺口，提案在左侧 AI 工作稿里。"
                    )
                } else {
                    format!(
                        "{prefix}{skill}完成（{rounds} 轮）：补全 {resolved} 处，还有 {pending} 处待确认，{asked} 道题在侧栏里。"
                    )
                };
            }
            Err(error) => {
                doc.ai_review_baseline = None;
                doc.ai_panel.finish(TurnState::Failed(error.clone()));
                self.status = format!("{prefix}AI 任务失败：{error}");
            }
        }
    }

    /// 后台任务回投。稿件可能已经关闭，或者同一篇又发起了新任务——
    /// 两种情况下这份结果都已作废，直接丢掉，绝不能落到别的稿件上。
    pub(crate) fn apply_doc_job(&mut self, key: DocKey, seq: u64, job: DocJob) {
        let Some(index) = self.docs.iter().position(|doc| doc.key == key) else {
            return;
        };
        // 停止会使 job_seq 失效；只读的用量仍可回到原轮次，其余产物照旧丢弃。
        if let DocJob::AiUsage { turn_id, usage } = job {
            self.docs[index].ai_panel.record_usage(turn_id, seq, usage);
            return;
        }
        if self.docs[index].job_seq != seq {
            return;
        }
        if !matches!(
            job,
            DocJob::ExportProgress(_)
                | DocJob::AiStream { .. }
                | DocJob::AiNote(_)
                | DocJob::AiTool(_)
                | DocJob::AiWorkspace(_)
                | DocJob::AiWriteBegin { .. }
                | DocJob::AiSessionSummary { .. }
                | DocJob::StyleUsed(_)
        ) {
            self.docs[index].busy = false;
        }
        // 后台跑完的未必是当前显示的那篇，状态栏要点名是谁。
        let prefix = if index == self.active_doc {
            String::new()
        } else {
            format!("《{}》", self.docs[index].title())
        };
        match job {
            DocJob::Reviewed(Ok(outcome)) => {
                // 缓存合并而不是覆盖：这一轮跳过的句子，结论还在旧缓存里。
                self.docs[index].revise_cache.extend(outcome.cache);
                for (task, count) in &outcome.rejected_by_task {
                    self.metrics.record_gate_rejection(task, *count);
                }
                for ((task, reason), count) in &outcome.rejected_by_reason {
                    self.metrics.record_gate_reason(task, *reason, *count);
                }
                crate::metrics::save(&mut self.metrics);
                let markdown = std::mem::take(&mut self.docs[index].generated_markdown);
                let ignored = std::mem::take(&mut self.config.proofread.ignored);
                let stale = self.docs[index].revisions.replace_model(
                    outcome.suggestions,
                    &markdown,
                    &ignored,
                );
                self.config.proofread.ignored = ignored;
                self.docs[index].generated_markdown = markdown;
                let found = self.docs[index].revisions.model_count();
                // 三个数字都要如实说。拦截数一直居高不下，说明这个检查器或这个
                // 模型不合用，该让用户知道并关掉它；失效数不为零，说明复核期间
                // 正文被改过、结果只是部分有效。静默吞掉这两种情况，用户只会
                // 以为模型什么都没查出来。
                let mut message = format!("{prefix}文字复核完成");
                if found == 0 {
                    message.push_str("，未发现可提交的问题");
                } else {
                    message.push_str(&format!("，{found} 条待确认，需人工逐条判断"));
                }
                if outcome.rejected > 0 {
                    message.push_str(&format!("；另有 {} 条未通过复核已丢弃", outcome.rejected));
                }
                if stale > 0 {
                    message.push_str(&format!(
                        "；{stale} 条因正文在复核期间被改动而失效，请重新复核"
                    ));
                }
                message.push('。');
                self.status = message;
                self.docs[index].result_drawer_open = true;
            }
            DocJob::Reviewed(Err(error)) => {
                self.status = format!("{prefix}文字复核失败：{error}");
            }
            DocJob::AiStream {
                content,
                reasoning,
                done,
            } => self.docs[index].ai_panel.append(&content, &reasoning, done),
            DocJob::AiTool(line) => self.docs[index].ai_panel.step(line),
            DocJob::AiUsage { .. } => unreachable!("用量已按原轮次回投"),
            DocJob::AiWorkspace(text) => self.docs[index].ai_panel.replace_content(text),
            DocJob::AiWriteBegin { prefix, suffix } => {
                self.docs[index].ai_panel.begin_write(prefix, suffix)
            }
            DocJob::SkillDone(result) => self.apply_skill_done(index, &prefix, result),
            DocJob::AiSessionSummary { summary, upto } => {
                let session = &mut self.docs[index].ai_panel.session;
                session.summary = summary;
                session.compacted_upto = upto;
            }
            DocJob::AiCompactDone(Ok((summary, upto, count))) => {
                let session = &mut self.docs[index].ai_panel.session;
                session.summary = summary;
                session.compacted_upto = upto;
                self.status =
                    format!("{prefix}会话已压缩：前面 {count} 轮并成了摘要，原始记录仍可翻看。");
            }
            DocJob::AiCompactDone(Err(error)) => {
                self.status = format!("{prefix}压缩会话失败：{error}");
            }
            DocJob::GapRevised(revision) => {
                let revision = *revision;
                crate::ai_panel::finish_research_revision(
                    &mut self.docs[index],
                    &self.config,
                    "按回答修订",
                    revision.before,
                    revision.research,
                    Some(revision.questions),
                );
                self.status = format!("{prefix}已按你的回答修订，新的提案在侧栏里。");
            }
            DocJob::StyleUsed(id) => {
                if let Ok(mut book) = crate::agent::style::StyleBook::load() {
                    book.record_use(&id);
                    let _ = book.save();
                }
            }
            DocJob::AiNote(note) => {
                self.status = format!("{prefix}{note}");
                self.docs[index].ai_panel.note(note);
            }
            DocJob::ExportProgress(message) => {
                self.docs[index].ai_panel.set_phase(&message);
                self.status = format!("{prefix}{message}");
            }
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
                // 要素问题不挡导出，但导出成功不等于这稿子能签发。数出来缀在状态栏
                // 末尾，免得「已导出 N 个文件」被读成「这份可以发了」。
                let mustfix = crate::validator::mustfix_issues(
                    &self.docs[index].draft,
                    &self.docs[index].generated_markdown,
                    &self.config.vocabulary,
                    &self.config.security_rules,
                )
                .len();
                let mustfix_tail = if mustfix > 0 {
                    format!("；仍有 {mustfix} 项要素问题待补齐，签发前请处理")
                } else {
                    String::new()
                };
                // 孤行、编译失败、要素未齐都是这一下才摆到眼前的，弹抽屉把它们顶出来，
                // 别让提示躺在折叠面板里看不见。
                if orphans > 0 || mustfix > 0 || self.docs[index].export_error.is_some() {
                    self.docs[index].result_drawer_open = true;
                }
                self.status = if self.docs[index].export_error.is_some() {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件；PDF 编译失败，请查看审校提示。",
                        self.docs[index].output_files.len()
                    )
                } else if orphans > 0 {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件；实测发现 {orphans} 处孤行，见审校提示{mustfix_tail}。",
                        self.docs[index].output_files.len()
                    )
                } else {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件{mustfix_tail}。",
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
            DocJob::RedlinePrintPreview(Ok(files)) => {
                match files
                    .into_iter()
                    .find(|file| file.extension().is_some_and(|ext| ext == "pdf"))
                {
                    Some(pdf) => {
                        self.status = format!("{prefix}花脸稿打印预览已生成。");
                        self.open_pdf(pdf, Some("花脸稿打印预览".to_string()));
                    }
                    None => self.status = format!("{prefix}花脸稿打印预览没有编出 PDF。"),
                }
            }
            DocJob::RedlinePrintPreview(Err(error)) => {
                self.status = format!("{prefix}花脸稿打印预览编译失败：{error}");
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

/// 结果摘要里的失败部分：「，N 篇失败（《甲》：原因；…）」，最多列三条；无失败为空串。
fn failure_summary(failed: &[(String, String)]) -> String {
    if failed.is_empty() {
        return String::new();
    }
    let detail = failed
        .iter()
        .take(3)
        .map(|(title, err)| format!("《{title}》：{err}"))
        .collect::<Vec<_>>()
        .join("；");
    let more = if failed.len() > 3 { "；…" } else { "" };
    format!("，{} 篇失败（{detail}{more}）", failed.len())
}
