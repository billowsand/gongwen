//! 后台任务通道、模型探测与知识库任务。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::jobs`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

#[cfg(feature = "ai-workbench")]
use crate::ai_panel::TurnState;
use crate::app::{ExportOutcome, GongwenApp, TabRef};
#[cfg(feature = "ai-workbench")]
use crate::app::{KnowledgeImportDraft, KnowledgePreviewState};
#[cfg(feature = "ai-workbench")]
use crate::doc_import;
use crate::draft_page::DocKey;
#[cfg(feature = "ai-workbench")]
use crate::draft_page::DraftSession;
#[cfg(feature = "ai-workbench")]
use crate::knowledge;
#[cfg(feature = "ai-workbench")]
use crate::lmstudio;
use crate::manuscript_io;
use crate::models::ReviewNote;
#[cfg(feature = "ai-workbench")]
use crate::models::{DraftInput, GeneratedDraft, ModelKind, RerankMode, TemplateKind};
use crate::pdf_viewer;
use crate::pdf_viewer::PdfKey;
#[cfg(feature = "ai-workbench")]
use crate::preview;
#[cfg(feature = "ai-workbench")]
use crate::qa;
#[cfg(feature = "ai-workbench")]
use crate::rag;
#[cfg(feature = "ai-workbench")]
use crate::storage;
use crate::system_fonts;
#[cfg(feature = "ai-workbench")]
use crate::theme;
#[cfg(feature = "ai-workbench")]
use crate::units::UnitDisplay;
use eframe::egui;
use std::path::PathBuf;
use std::thread;

pub(crate) enum WorkerResult {
    /// 外部程序把文件放进系统剪贴板的结果，后台线程执行。
    FileCopied {
        path: PathBuf,
        result: Result<(), String>,
    },
    /// 稿件包已复制到私有暂存目录；导入只在主线程继续排队。
    ImportPackageStaged(Result<PathBuf, String>),
    /// 探测一家提供商：连接结果与已加载模型清单。
    /// `probed_kinds` 是服务原生接口自报的模型用途（目前只有 LM Studio 给），
    /// 写回配置前还要过一遍 `classify_model` 补名字启发式。
    #[cfg(feature = "ai-workbench")]
    ProviderModels {
        provider_id: String,
        result: Result<Vec<String>, String>,
        probed_kinds: std::collections::HashMap<String, ModelKind>,
    },
    /// 真跑一次 rerank 的验证结果（端点路径 + 响应字段是否对得上）。
    #[cfg(feature = "ai-workbench")]
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
    /// 后台读取剪贴板并转换完成；真正的正文替换在主线程按粘贴快照执行。
    SmartPasteDone { key: DocKey },
    ExternalPaste {
        sequence: u64,
        result: Result<String, String>,
    },
    /// 知识库任务：与具体稿件无关的全局任务（索引构建 / 检索测试）。
    #[cfg(feature = "ai-workbench")]
    Knowledge(KnowledgeJob),
    /// 公文词表任务：语料扫描的进度与结果。
    Lexicon(crate::app::LexiconJob),
    /// 扫描本机字体目录的结果。中文字体文件很大，扫描放在后台线程。
    SystemFonts(Vec<system_fonts::SystemFont>),
    /// 关于窗的更新检查结果。
    UpdateChecked(Result<crate::update_check::ReleaseInfo, String>),
    /// 导出全量备份：目标路径与文件字节数。
    BackupExported {
        path: PathBuf,
        result: Result<u64, String>,
    },
    /// 校验备份文件：ZIP 路径与条目概要。
    BackupInspected {
        path: PathBuf,
        result: Result<crate::data_backup::BackupInfo, String>,
    },
    /// 恢复备份：源 ZIP 与恢复报告（留底目录、解出文件数）。
    BackupRestored {
        path: PathBuf,
        result: Result<crate::data_backup::RestoreReport, String>,
    },
    /// 送批材料合并导出的进度与结果。
    SendPackage(crate::app::SendPackageEvent),
    /// 固定快照的稿件包完成加密；只有成功后才持久化用户选择记住的密码。
    ManuscriptPackageExport {
        path: PathBuf,
        result: Result<manuscript_io::ExportSummary, String>,
        remembered_password: Option<String>,
    },
    /// 确认导入在后台完成；预览仍留在界面状态里，失败可恢复。
    ManuscriptImportDone {
        manifest_hash: String,
        result: Result<manuscript_io::sync::SyncImportSummary, String>,
    },
    /// 后台读取完用户选中的 PDF 附件；真正的入库仍在主线程。
    ManuscriptAttachmentsRead {
        job: crate::app::manuscript_ui::AttachmentReadJob,
        result: Result<Vec<(String, Vec<u8>)>, String>,
    },
    /// 稿件 PDF 批量导出的结果。`path` 是保存的 7z 路径。
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
    /// 打印对话框的打印机清单（后台线程查好）。`dialog` 对不上的是已关掉的对话框的。
    PrintersListed {
        dialog: u64,
        result: Result<Vec<crate::print::PrinterInfo>, String>,
    },
    /// 打印对话框里某台打印机的状态。
    PrinterStateChecked {
        dialog: u64,
        name: String,
        state: crate::print::PrinterState,
    },
    /// 驱动「打印机首选项」窗口的结果：改过的整块 DEVMODE；取消是 `Ok(None)`。
    /// 只有 Windows 会发出这条消息，但变体在所有平台都编译——对话框的逻辑不再分平台。
    PreferencesEdited {
        dialog: u64,
        printer: String,
        result: Result<Option<Vec<u8>>, String>,
    },
    /// 打印进度：已送页数 / 总页数（Windows 每页一次，CUPS 不报）。
    PrintProgress { dialog: u64, done: u32, total: u32 },
    /// 打印对话框打开时从 PDF 自身测得的版式事实。读不出来是 `None`。
    PdfInspected {
        dialog: u64,
        facts: Option<crate::print::PdfFacts>,
    },
    /// 打印对话框里某台打印机的能力（双面、颜色、纸张、纸盒）。
    PrinterCapsChecked {
        dialog: u64,
        name: String,
        caps: crate::print::PrinterCaps,
    },
    /// PDF 打印任务（后台线程逐页光栅化或交给 `lp`）的结果。
    PdfPrinted {
        path: PathBuf,
        /// 送到哪台打印机；`None` 是系统默认或系统对话框里选的。
        printer: Option<String>,
        /// 打完不关的对话框（手动双面的两轮）；普通的打印对话框提交时就关了。
        dialog: Option<u64>,
        result: Result<crate::print::PrintOutcome, String>,
    },
    /// Pi 工作区占用统计结果：总字节数与轮次数。
    #[cfg(feature = "ai-workbench")]
    PiWorkspaceUsage(Result<(u64, usize), String>),
    /// 清理旧 Pi 工作区：清掉几轮、释放多少字节。
    #[cfg(feature = "ai-workbench")]
    PiWorkspacePruned(Result<(usize, u64), String>),
}

/// 知识库后台任务的结果。
#[cfg(feature = "ai-workbench")]
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
        auto_enabled: bool,
        auto_materials: Vec<(String, String, String)>,
    },
    /// 索引整批完成。`failed` 是 (标题, 错误)。
    IndexFinished {
        ok: usize,
        failed: Vec<(String, String)>,
    },
    /// 检索测试的结果。
    SearchDone {
        query: String,
        kind: Option<TemplateKind>,
        result: Result<rag::RetrievalOutcome, String>,
    },
    /// 本地全文检索结果，携带原查询用于丢弃过期响应。
    LocalSearchDone {
        query: String,
        kind: Option<TemplateKind>,
        result: Result<Vec<crate::knowledge::LocalSearchHit>, String>,
    },
    /// 旧文档全文索引补建结束。
    LocalBackfillDone {
        ok: usize,
        failed: Vec<(String, String)>,
    },
    /// 知识库问答的结果。`question` 是发起时的原始提问，回来时连同答案一起入历史。
    QaDone {
        question: String,
        result: Result<qa::QaOutcome, String>,
    },
}

/// 知识库页输入区的模式：检索片段列表，还是基于片段生成问答答案。
/// 两种模式共用同一个输入框，切换只改变「发送后干什么」，互不清空各自结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg(feature = "ai-workbench")]
pub(crate) enum KnowledgeMode {
    #[default]
    Search,
    Qa,
}

/// 起草页发起的后台任务的结果。
pub(crate) enum DocJob {
    /// 模型的流式增量，落进 AI 侧栏正在跑的那一轮，不碰正文。`done` 表示模型
    /// 已经说完、接下来是程序校验。
    #[cfg(feature = "ai-workbench")]
    AiStream {
        content: String,
        reasoning: String,
        done: bool,
    },
    /// 要留在 AI 侧栏卡片上的说明（知识库命中了哪几篇、为什么没命中）。
    #[cfg(feature = "ai-workbench")]
    AiNote(String),
    /// 动笔前「要素抽取」直接给出的要素建议：挂到这一轮的建议卡上（不挡起草）。
    #[cfg(feature = "ai-workbench")]
    AiFieldSuggestions(Vec<crate::element_fields::FieldSuggestion>),
    /// 技能流程的一次工具调用，侧栏「过程」里一行。
    #[cfg(feature = "ai-workbench")]
    AiTool(String),
    /// Pi 智能写作的进度（读正文、等模型、工具完成……）。
    #[cfg(feature = "ai-workbench")]
    PiProgress(String),
    /// Pi 本轮的一次工具调用，侧栏「过程」里一行。
    #[cfg(feature = "ai-workbench")]
    PiTool(String),
    #[cfg(feature = "ai-workbench")]
    PiActivity(crate::pi_agent::PiActivity),
    /// Pi 工作区有已验证的候选稿，可作为缺少阶段信息时的进度提示。
    #[cfg(feature = "ai-workbench")]
    PiCandidate,
    /// Pi 本轮结束。成功时是一份固定版本或侧栏清单，失败时报原因；
    /// 未提交、超时、取消、崩溃都不会走到成功分支。
    /// Pi 交付结果。装箱：`PiOutcome` 带着完整正文，不把整个 `DocJob` 撞大。
    #[cfg(feature = "ai-workbench")]
    PiDone(Box<Result<crate::pi_agent::PiOutcome, String>>),
    /// Pi 在等用户回答；题目只活在本轮。
    #[cfg(feature = "ai-workbench")]
    PiQuestions(crate::pi_agent::PiQuestionBatch),
    /// Pi 后台准备完成：把稳定运行引用交回界面，等界面核对轮次、保存会话后确认。
    #[cfg(feature = "ai-workbench")]
    PiPrepared {
        turn_id: u64,
        reference: crate::pi_agent::PiRunRef,
        dev_runtime: bool,
        bridge: std::sync::Arc<crate::pi_agent::PiBridge>,
        kind: String,
        ack: crate::ai_panel::pi_prepare::PiPrepareAck,
    },
    #[cfg(feature = "ai-workbench")]
    AiUsage {
        turn_id: u64,
        usage: crate::ai_services::backend::UsageTotals,
    },
    /// 技能流程的工作稿整体换新（补全之后）。
    #[cfg(feature = "ai-workbench")]
    AiWorkspace(String),
    #[cfg(feature = "ai-workbench")]
    AiWriteBegin {
        prefix: String,
        suffix: String,
    },
    /// 技能任务结束：流程挂起要问的题，或定稿后的提案。
    #[cfg(feature = "ai-workbench")]
    SkillDone(Result<Box<crate::ai_panel::SkillResult>, String>),
    /// 技能任务开跑前压缩了会话：新的会话摘要与它覆盖到的最后一轮。
    #[cfg(feature = "ai-workbench")]
    AiSessionSummary {
        summary: String,
        upto: u64,
    },
    /// 手动压缩会话的结果：(摘要, 覆盖到的最后一轮, 并进去几轮)。
    #[cfg(feature = "ai-workbench")]
    AiCompactDone(Result<(String, u64, usize), String>),
    /// 这一轮用了哪份风格档案（记一次使用）。
    #[cfg(feature = "ai-workbench")]
    StyleUsed(String),
    /// 按回答修订的结果：AI 把缺口题的回答写进了所在段落（或闸门不过、直接替换）。
    #[cfg(feature = "ai-workbench")]
    GapRevised(Box<crate::ai_panel::GapRevision>),
    ExportProgress(String),
    Exported(Result<ExportOutcome, String>),
    /// 花脸稿导出结果。与定稿导出分开：花脸稿不是成品，不该顶掉工具栏上
    /// 「打开最近导出」指向的定稿文件。
    RedlineExported(Result<Vec<std::path::PathBuf>, String>),
    /// 花脸稿打印预览：只出 PDF、落在临时目录，编完在内置查看器里打开。
    RedlinePrintPreview(Result<Vec<u8>, String>),
    MemoryPrintPdf(Result<Vec<u8>, String>),
    /// 小模型逐句文字复核的结果。只产出待确认的建议，不碰正文。
    #[cfg(feature = "ai-workbench")]
    Reviewed(Result<crate::revise_model::ReviewOutcome, String>),
}

impl GongwenApp {
    pub(crate) fn start_file_copy(&mut self, path: PathBuf, wake: egui::Context) {
        self.status = format!(
            "正在复制 {} 到系统剪贴板…",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("文件")
        );
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = crate::file_clipboard::copy_file(&path);
            let _ = tx.send(WorkerResult::FileCopied { path, result });
            wake.request_repaint();
        });
    }

    /// 探测一家提供商：测试连接并把已加载模型清单缓存进配置。
    ///
    /// 各家独立进行，互不阻塞；状态挂在提供商卡片上（`provider_status`）。
    #[cfg(feature = "ai-workbench")]
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
    #[cfg(feature = "ai-workbench")]
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

    /// 检查最新发布。只发一次请求，结果回到关于窗内联显示。
    pub(crate) fn start_update_check(&mut self, ctx: &egui::Context) {
        if matches!(
            self.update_check,
            crate::update_check::UpdateCheckState::Checking
        ) {
            return;
        }
        let url = self.config.update_check_url.trim().to_string();
        if url.is_empty() {
            self.update_check = crate::update_check::UpdateCheckState::Failed(
                "更新检查地址为空，请到设置页填写".into(),
            );
            return;
        }
        self.update_check = crate::update_check::UpdateCheckState::Checking;
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = crate::update_check::fetch_latest(&url, std::time::Duration::from_secs(5))
                .and_then(|body| crate::update_check::parse_latest_release(&body))
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::UpdateChecked(result));
            ctx.request_repaint();
        });
    }

    /// 真跑一次 rerank，验证端点路径与响应字段是否对得上。
    ///
    /// 只查 `/v1/models` 是不够的：有些服务（如 LM Studio）对不认识的路径会记
    /// `Unexpected endpoint or method` 却仍返回 200，于是"连接成功"，
    /// 而每次检索的 rerank 都在静默失败。
    #[cfg(feature = "ai-workbench")]
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
        let chat = match self.config.aux_chat() {
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
                #[cfg(feature = "ai-workbench")]
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
                            // 只有一个模型且写作模型还没选时自动选上，省去手选。
                            if self.config.pi_model.is_empty() && models.len() == 1 {
                                self.config.pi_model = crate::models::ModelRef {
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
                WorkerResult::FileCopied { path, result } => {
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("文件");
                    self.status = match result {
                        Ok(()) => format!("已复制到剪贴板 {name}。"),
                        Err(error) => format!("复制到剪贴板失败：{error}"),
                    };
                }
                WorkerResult::ImportPackageStaged(result) => {
                    self.package_staging_busy = false;
                    ctx.request_repaint();
                    match result {
                        Ok(path) => self.pending_imports.push_back(path),
                        Err(error) => self.status = format!("无法导入稿件包：{error}"),
                    }
                    self.pump_package_staging();
                }
                WorkerResult::SmartPasteDone { key } => {
                    let Some(index) = self.doc_index_of_key(key) else {
                        continue;
                    };
                    let Some(pending) = self.docs[index].pending_paste.take() else {
                        continue;
                    };
                    if self.tabs.get(self.active_tab) != Some(&TabRef::Doc(key))
                        || ctx.input(|input| !input.focused)
                    {
                        continue;
                    }
                    let result = pending
                        .receiver
                        .try_recv()
                        .unwrap_or_else(|error| Err(format!("无法读取粘贴结果：{error}")));
                    self.draft_page_at(index).finish_paste(ctx, pending, result);
                }
                WorkerResult::ExternalPaste { sequence, result } => {
                    self.finish_external_paste(ctx, sequence, result);
                }
                WorkerResult::Doc { key, seq, job } => {
                    self.apply_doc_job_with(Some(ctx), key, seq, job)
                }
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
                #[cfg(feature = "ai-workbench")]
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
                WorkerResult::UpdateChecked(result) => {
                    self.update_check = match result {
                        Ok(info) => {
                            if crate::update_check::compare_versions(
                                env!("CARGO_PKG_VERSION"),
                                &info.version,
                            ) == std::cmp::Ordering::Less
                            {
                                crate::update_check::UpdateCheckState::Available(info)
                            } else {
                                crate::update_check::UpdateCheckState::Latest
                            }
                        }
                        Err(error) => crate::update_check::UpdateCheckState::Failed(error),
                    };
                }
                WorkerResult::BackupExported { path, result } => {
                    self.backup_busy = false;
                    self.status = match result {
                        Ok(bytes) => format!(
                            "备份完成：{}（{}）。",
                            path.display(),
                            crate::data_backup::format_bytes(bytes)
                        ),
                        Err(error) => format!("导出备份失败：{error}"),
                    };
                }
                WorkerResult::BackupInspected { path, result } => {
                    self.backup_busy = false;
                    match result {
                        Ok(info) => {
                            self.status = format!(
                                "已校验备份：{} 个文件，共 {}。请确认后恢复。",
                                info.file_count,
                                crate::data_backup::format_bytes(info.total_bytes)
                            );
                            self.backup_restore_prompt = Some((path, info));
                        }
                        Err(error) => {
                            self.backup_restore_password = None;
                            self.status = format!("备份文件校验失败：{error}");
                            self.backup_restore_error =
                                Some(format!("无法恢复这个文件：\n{error}"));
                        }
                    }
                }
                WorkerResult::BackupRestored { path, result } => {
                    self.backup_busy = false;
                    self.backup_restore_running = false;
                    match result {
                        Ok(report) => {
                            self.status = format!(
                                "已从 {} 恢复 {} 个文件。为避免用旧密钥覆盖恢复的数据，                                 正在退回锁屏，请重新解锁（使用备份时的应用密码）。",
                                path.display(),
                                report.restored_files
                            );
                            self.backup_restore_result = Some(report);
                            // 数据目录已被整体替换：当前句柄与连接全部过期。
                            crate::vault::clear_key_handle();
                            self.request_force_relock();
                        }
                        Err(error) => {
                            // 恢复中途失败时数据已按留底目录回滚；连接要重新打开，
                            // 否则界面会一直停在「稿件库不可用」。
                            self.reopen_local_stores();
                            self.status = format!("恢复备份失败：{error}");
                            self.backup_restore_error = Some(error);
                        }
                    }
                }
                #[cfg(feature = "ai-workbench")]
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
                WorkerResult::ManuscriptPackageExport {
                    path,
                    result,
                    remembered_password,
                } => {
                    self.manuscript_package_export_busy = false;
                    match result {
                        Ok(summary) => {
                            self.status = format!(
                                "已导出 {} 篇稿件、{} 个 PDF 附件到 {}。",
                                summary.records,
                                summary.pdfs,
                                path.display()
                            );
                            self.update_remembered_zip_password(remembered_password);
                        }
                        Err(error) => self.status = format!("导出稿件包失败：{error}"),
                    }
                }
                WorkerResult::ManuscriptImportDone {
                    manifest_hash,
                    result,
                } => self.apply_manuscript_import_done(manifest_hash, result),
                WorkerResult::ManuscriptAttachmentsRead { job, result } => {
                    self.apply_attachment_read(job, result);
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
                WorkerResult::PrintersListed { dialog, result } => {
                    self.apply_printers_listed(dialog, result);
                }
                WorkerResult::PrinterStateChecked {
                    dialog,
                    name,
                    state,
                } => self.apply_printer_state(dialog, &name, state),
                WorkerResult::PrinterCapsChecked { dialog, name, caps } => {
                    self.apply_printer_caps(dialog, &name, caps)
                }
                WorkerResult::PdfInspected { dialog, facts } => {
                    self.apply_pdf_inspected(dialog, facts)
                }
                WorkerResult::PrintProgress {
                    dialog,
                    done,
                    total,
                } => self.apply_print_progress(dialog, done, total),
                WorkerResult::PreferencesEdited {
                    dialog,
                    printer,
                    result,
                } => self.apply_preferences_edited(dialog, &printer, result),
                WorkerResult::PdfPrinted {
                    path,
                    printer,
                    dialog,
                    result,
                } => self.apply_pdf_printed(path, printer, dialog, result),
                #[cfg(feature = "ai-workbench")]
                WorkerResult::PiWorkspaceUsage(result) => {
                    self.pi_workspace_busy = false;
                    match result {
                        Ok((bytes, runs)) => {
                            self.pi_workspace_usage = Some((bytes, runs));
                            self.pi_workspace_error = None;
                        }
                        Err(error) => {
                            // 留个失败标记，别每帧重新起线程。
                            self.pi_workspace_usage = None;
                            self.pi_workspace_error = Some(error.clone());
                            self.status = format!("读取 Pi 工作区占用失败：{error}");
                        }
                    }
                }
                #[cfg(feature = "ai-workbench")]
                WorkerResult::PiWorkspacePruned(result) => {
                    self.pi_workspace_busy = false;
                    // 清完让下一次进页面重新统计，看到的是清理后的占用。
                    self.pi_workspace_usage = None;
                    match result {
                        Ok((pruned, bytes)) => {
                            self.status = format!(
                                "已清理 {pruned} 个旧工作区，释放 {} MB（只删已写入正文的旧轮次和孤儿目录）",
                                bytes / (1024 * 1024)
                            );
                        }
                        Err(error) => {
                            self.status = format!("清理 Pi 工作区失败：{error}");
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    /// 知识库后台任务的收尾。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn apply_knowledge_job(&mut self, job: KnowledgeJob) {
        match job {
            KnowledgeJob::IndexProgress {
                done,
                total,
                current_title,
            } => {
                self.knowledge_index_progress = Some((done, total, current_title));
            }
            KnowledgeJob::ImportFinished {
                ok,
                failed,
                auto_enabled,
                auto_materials,
            } => {
                self.knowledge_busy = false;
                self.knowledge_index_progress = None;
                self.knowledge_dirty = true;
                let next = if ok == 0 {
                    String::new()
                } else {
                    "，本地全文索引已就绪；语义检索可另建向量索引".to_string()
                };
                self.knowledge_index_result = Some(format!(
                    "导入完成：{ok} 篇成功{next}{}。",
                    failure_summary(&failed)
                ));
                if auto_enabled {
                    if ok > auto_materials.len() {
                        self.task_ui.extraction_error = Some(format!(
                            "{} 篇材料超过自动整理上限，已保存在知识库，请分批手工整理。",
                            ok - auto_materials.len()
                        ));
                    }
                    self.enqueue_task_materials(auto_materials);
                }
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
            KnowledgeJob::SearchDone {
                query,
                kind,
                result: Ok(outcome),
            } if self.knowledge_test_query.trim() == query
                && self.knowledge_filter_kind == kind =>
            {
                self.knowledge_busy = false;
                self.knowledge_test_results = outcome.chunks;
                // 降级说明直接摆在检索区，不再只写进服务端日志。
                self.knowledge_search_warnings = outcome.warnings;
            }
            KnowledgeJob::SearchDone {
                query,
                kind,
                result: Err(error),
            } if self.knowledge_test_query.trim() == query
                && self.knowledge_filter_kind == kind =>
            {
                self.knowledge_busy = false;
                self.knowledge_test_results = Vec::new();
                self.knowledge_search_warnings = Vec::new();
                self.knowledge_error = Some(format!("知识库检索失败：{error}"));
            }
            KnowledgeJob::SearchDone { .. } => {
                self.knowledge_busy = false;
            }
            KnowledgeJob::LocalSearchDone {
                query,
                kind,
                result,
            } => {
                self.knowledge_busy = false;
                if self.knowledge_test_query.trim() == query && self.knowledge_filter_kind == kind {
                    match result {
                        Ok(hits) => self.knowledge_local_results = hits,
                        Err(error) => {
                            self.knowledge_local_results.clear();
                            self.knowledge_error = Some(format!("本地全文检索失败：{error}"));
                        }
                    }
                }
            }
            KnowledgeJob::LocalBackfillDone { ok, failed } => {
                self.knowledge_busy = false;
                self.knowledge_index_progress = None;
                self.knowledge_index_result = Some(if failed.is_empty() {
                    format!("本地全文索引就绪：补建 {ok} 篇旧文档。")
                } else {
                    format!(
                        "本地全文索引补建：{ok} 篇成功{}。",
                        failure_summary(&failed)
                    )
                });
            }
            KnowledgeJob::QaDone { question, result } => {
                self.knowledge_busy = false;
                self.knowledge_qa_pending = None;
                match result {
                    Ok(outcome) => self.knowledge_qa_history.push(qa::QaTurn {
                        question,
                        answer: outcome.answer,
                        references: outcome.references,
                        wiki_pages: outcome.wiki_pages,
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
    #[cfg(feature = "ai-workbench")]
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
        // 维基侧派生状态：资料「已用于维基」标记 + 新资料关联提示。
        // store 的可变借用到此为止（NLL），下面可以安全改 self。
        self.refresh_wiki_doc_status();
        self.detect_new_docs_for_wiki();
    }

    /// 库内是否存在与当前配置不同的 embedding 模型。换模型后维度多半不同，
    /// 旧块的余弦恒为 0，会静默退出向量召回——必须提示用户重建索引。
    #[cfg(feature = "ai-workbench")]
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
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn knowledge_pick_documents(&mut self) {
        self.request_files(doc_import::knowledge_files_spec(), |app, paths| {
            let Some(paths) = paths else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            app.knowledge_import = Some(KnowledgeImportDraft {
                paths,
                kind: app.config.last_template,
            });
        });
    }

    /// 确认导入外部文档：后台转成 Markdown 入库，同时建立本地全文索引。
    ///
    /// 导入与全文索引均不依赖 embedding；语义检索需要的向量索引另行建立。
    /// 转换放进后台：单个 docx 是毫秒级，
    /// 但一次选几十个文件、或几百页的 PDF 就会卡住界面。失败（扫描版 PDF、加密、
    /// 格式坏了）逐个记下，汇总在结果摘要里，不让后一条把前一条覆盖掉。
    #[cfg(feature = "ai-workbench")]
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
        let auto_enabled = match self.task_store.as_ref().map(|s| s.auto()) {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                self.task_ui.extraction_error = Some(format!("读取自动整理开关失败：{e:#}"));
                false
            }
            None => false,
        };
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
            let mut auto_materials = Vec::new();
            let mut store = match knowledge::KnowledgeStore::open(&db_path) {
                Ok(store) => store,
                Err(error) => {
                    failed.push(("知识库".into(), format!("打开知识库失败：{error:#}")));
                    let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::ImportFinished {
                        ok,
                        failed,
                        auto_enabled,
                        auto_materials,
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
                    title: knowledge::import_title(&content, &stem),
                    content_markdown: content,
                };
                match store.import_document(&item) {
                    Ok(id) => {
                        match store.index_local_document(id) {
                            Ok(true) => ok += 1,
                            Ok(false) => {
                                failed.push((name.clone(), "导入完成后文档已被移除".into()))
                            }
                            Err(error) => {
                                failed.push((name.clone(), format!("全文索引失败：{error:#}")))
                            }
                        }
                        if auto_enabled
                            && super::task_assistant::can_add_material(
                                &auto_materials,
                                &item.content_markdown,
                            )
                        {
                            auto_materials.push((
                                item.title.clone(),
                                item.content_markdown.clone(),
                                format!("knowledge:{}", item.source_path),
                            ));
                        }
                    }
                    Err(error) => failed.push((name, format!("写入知识库失败：{error:#}"))),
                }
            }
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::ImportFinished {
                ok,
                failed,
                auto_enabled,
                auto_materials,
            }));
        });
    }

    /// 把稿件库当前勾选的稿件加入知识库（只入库，不建索引）。稿件正文已在库里，
    /// 写一遍是毫秒级，直接在主线程做。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn knowledge_import_selected_manuscripts(&mut self) {
        let auto_enabled = match self.task_store.as_ref().map(|s| s.auto()) {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                self.task_ui.extraction_error = Some(format!("读取自动整理开关失败：{e:#}"));
                false
            }
            None => false,
        };
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
            match manuscripts.get_for_editor(id) {
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
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            self.status = "知识库路径不可用。".into();
            return;
        };
        if self.knowledge_busy {
            return;
        }
        self.knowledge_busy = true;
        self.knowledge_progress_verb = "正在导入并建立全文索引";
        self.knowledge_index_progress = Some((0, items.len(), String::new()));
        let tx = self.sender.clone();
        thread::spawn(move || {
            let mut ok = 0usize;
            let mut auto_materials = Vec::new();
            let mut store = match knowledge::KnowledgeStore::open(&db_path) {
                Ok(store) => store,
                Err(error) => {
                    failed.push(("知识库".into(), format!("{error:#}")));
                    let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::ImportFinished {
                        ok,
                        failed,
                        auto_enabled,
                        auto_materials,
                    }));
                    return;
                }
            };
            let total = items.len();
            for (index, item) in items.iter().enumerate() {
                let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::IndexProgress {
                    done: index,
                    total,
                    current_title: item.title.clone(),
                }));
                match store.import_document(item) {
                    Ok(id) => match store.index_local_document(id) {
                        Ok(true) => {
                            ok += 1;
                            if auto_enabled
                                && super::task_assistant::can_add_material(
                                    &auto_materials,
                                    &item.content_markdown,
                                )
                            {
                                auto_materials.push((
                                    item.title.clone(),
                                    item.content_markdown.clone(),
                                    format!(
                                        "manuscript:{}",
                                        item.source_manuscript_id.unwrap_or_default()
                                    ),
                                ));
                            }
                        }
                        Ok(false) => failed.push((item.title.clone(), "文档已被移除".into())),
                        Err(error) => {
                            failed.push((item.title.clone(), format!("全文索引失败：{error:#}")))
                        }
                    },
                    Err(error) => failed.push((item.title.clone(), format!("{error:#}"))),
                }
            }
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::ImportFinished {
                ok,
                failed,
                auto_enabled,
                auto_materials,
            }));
        });
    }

    /// 为已导入、尚未索引的文档建立索引。
    #[cfg(feature = "ai-workbench")]
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
    #[cfg(feature = "ai-workbench")]
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
    #[cfg(feature = "ai-workbench")]
    fn start_knowledge_index(&mut self, doc_ids: Vec<i64>) {
        if self.knowledge_busy {
            self.knowledge_error = Some("知识库任务正在进行中，请稍候。".into());
            return;
        }
        if self.config.embedding_model_name().is_empty() {
            self.knowledge_error = Some(
                "还没有配置 embedding 模型，无法建立索引：请先到「AI 管理 → 模型服务」的资料库检索块选择。文档已在库内，配置好后再点「建立索引」即可。"
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
    #[cfg(feature = "ai-workbench")]
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
        self.knowledge_local_results.clear();
        self.knowledge_test_results.clear();
        self.knowledge_error = None;
        self.knowledge_search_warnings.clear();
        let cfg = self.config.resolved_rag();
        // 重排走「对话大模型」模式时要用到聊天模型的配置。
        let chat = self.config.aux_chat().unwrap_or_default();
        let kind = self.knowledge_filter_kind;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = rag::retrieve(&cfg, &chat, &db_path, &query, kind)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::SearchDone {
                query,
                kind,
                result,
            }));
        });
    }

    /// 在后台线程跑一次知识库问答：检索片段 → 生成答案。问题带回时连同答案
    /// 一起入历史，等待期间 `knowledge_qa_pending` 记录原问题供对话区展示。
    #[cfg(feature = "ai-workbench")]
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
        let chat = self.config.aux_chat().unwrap_or_default();
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
    /// 删除知识库文档（单篇或批量），并从勾选里去掉。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn knowledge_delete(&mut self, ids: &[i64]) {
        if let Some(store) = self.knowledge_store.as_mut() {
            match store.delete_documents(ids) {
                Ok(removed) => {
                    self.status = format!("已从知识库删除 {removed} 篇。");
                    for id in ids {
                        self.knowledge_list.selected.remove(id);
                    }
                    self.knowledge_dirty = true;
                    self.knowledge_local_results.clear();
                    self.knowledge_test_results.clear();
                }
                Err(error) => self.status = format!("删除失败：{error:#}"),
            }
        }
        self.knowledge_delete_confirm = None;
    }

    /// 改知识库文档标题；成功后该篇转为待索引（理由见 `rename_document`）。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn knowledge_rename(&mut self, id: i64, title: &str) {
        if let Some(store) = self.knowledge_store.as_mut() {
            match store.rename_document(id, title) {
                Ok(()) => {
                    self.status = "标题已修改；全文索引将自动更新，向量索引可另行重建。".into();
                    self.knowledge_dirty = true;
                    self.knowledge_local_backfill_started = false;
                    self.knowledge_local_results.clear();
                    self.knowledge_test_results.clear();
                }
                Err(error) => self.status = format!("改标题失败：{error:#}"),
            }
        }
        self.knowledge_list.rename = None;
    }

    /// 打开知识库文档预览弹窗：从库里读出标题、文种、原文。
    #[cfg(feature = "ai-workbench")]
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
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn knowledge_preview_window(&mut self, ctx: &egui::Context) {
        let Some(preview) = self.knowledge_preview.as_mut() else {
            theme::reset_window_anim(ctx, egui::Id::unique("knowledge_preview_anim"));
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
            theme::window_enter_anim(ctx, egui::Id::unique("knowledge_preview_anim"), &w.response);
        }
        if !open || close_clicked {
            self.knowledge_preview = None;
        }
    }

    /// 生成与优化的收尾一致：换正文、换审校结果，有提示就弹审校抽屉。
    #[cfg(feature = "ai-workbench")]
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
    #[cfg(feature = "ai-workbench")]
    fn apply_skill_done(
        &mut self,
        index: usize,
        prefix: &str,
        result: Result<Box<crate::ai_panel::SkillResult>, String>,
    ) {
        #[cfg(feature = "ai-workbench")]
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
                let mut draft = draft;
                let before = doc
                    .ai_review_baseline
                    .take()
                    .unwrap_or_else(|| doc.generated_markdown.clone());
                let turn_id = doc.ai_panel.running_turn_mut().map(|turn| {
                    turn.provenance = crate::ai_panel::proposal_source(
                        &skill,
                        &turn.usage.draft_models(),
                        &report.cited_ids,
                        &report.generated_at,
                    );
                    // 自动选技能时卡片抬头先写着「自动选择技能」，定下来后换成技能名。
                    if turn.title.starts_with("自动选择技能") {
                        turn.title = turn.title.replacen("自动选择技能", &skill, 1);
                    }
                    turn.id
                });
                // 交付时还有待采纳的要素建议：提案提醒里点名（正文是按它们写的，
                // 表单没采纳就对不上）。按当前表单比对，已填成建议值的不算。
                if let Some(reminder) =
                    turn_id
                        .and_then(|id| doc.ai_panel.turn_mut(id))
                        .and_then(|turn| {
                            crate::ai_panel::field_card::proposal_reminder(
                                &turn.field_cards,
                                &doc.draft,
                            )
                        })
                {
                    draft.warnings.push(ReviewNote::from(reminder));
                }
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

    /// 后台任务回投（测试用：不带 `egui::Context`，Pi 成品直接换文本、不碰编辑器撤销栈）。
    #[cfg(test)]
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn apply_doc_job(&mut self, key: DocKey, seq: u64, job: DocJob) {
        self.apply_doc_job_with(None, key, seq, job);
    }

    /// 后台任务回投。稿件可能已经关闭，或者同一篇又发起了新任务——
    /// 两种情况下这份结果都已作废，直接丢掉，绝不能落到别的稿件上。
    ///
    /// 额外带上这一帧的 `egui::Context`：Pi 成品直接写入正文时，只有活动标签
    /// 才能动共用 `editor_id` 的撤销栈，需要它才能把一次写入记成可撤销的一步。
    pub(crate) fn apply_doc_job_with(
        &mut self,
        ctx: Option<&egui::Context>,
        key: DocKey,
        seq: u64,
        job: DocJob,
    ) {
        #[cfg(not(feature = "ai-workbench"))]
        let _ = ctx;
        let Some(index) = self.docs.iter().position(|doc| doc.key == key) else {
            return;
        };
        // 停止会使 job_seq 失效；只读的用量仍可回到原轮次，其余产物照旧丢弃。
        #[cfg(feature = "ai-workbench")]
        if let DocJob::AiUsage { turn_id, usage } = job {
            self.docs[index].ai_panel.record_usage(turn_id, seq, usage);
            return;
        }
        if self.docs[index].job_seq != seq {
            return;
        }
        #[cfg(feature = "ai-workbench")]
        if !matches!(
            job,
            DocJob::ExportProgress(_)
                | DocJob::AiStream { .. }
                | DocJob::AiNote(_)
                | DocJob::AiTool(_)
                | DocJob::PiProgress(_)
                | DocJob::PiTool(_)
                | DocJob::PiActivity(_)
                | DocJob::PiCandidate
                | DocJob::PiQuestions(_)
                | DocJob::PiPrepared { .. }
                | DocJob::AiWorkspace(_)
                | DocJob::AiWriteBegin { .. }
                | DocJob::AiSessionSummary { .. }
                | DocJob::AiFieldSuggestions(_)
                | DocJob::StyleUsed(_)
        ) {
            self.docs[index].busy = false;
        }
        #[cfg(not(feature = "ai-workbench"))]
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
            #[cfg(feature = "ai-workbench")]
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
            #[cfg(feature = "ai-workbench")]
            DocJob::Reviewed(Err(error)) => {
                self.status = format!("{prefix}文字复核失败：{error}");
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::AiStream {
                content,
                reasoning,
                done,
            } => self.docs[index].ai_panel.append(&content, &reasoning, done),
            #[cfg(feature = "ai-workbench")]
            DocJob::AiTool(line) => self.docs[index].ai_panel.step(line),
            #[cfg(feature = "ai-workbench")]
            DocJob::PiProgress(message) => {
                self.docs[index].ai_panel.pi_progress(&message);
                self.status = format!("{prefix}{message}");
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::PiTool(line) => self.docs[index].ai_panel.step(line),
            #[cfg(feature = "ai-workbench")]
            DocJob::PiActivity(activity) => self.docs[index].ai_panel.pi_activity(activity),
            #[cfg(feature = "ai-workbench")]
            DocJob::PiCandidate => {
                // 候选扫描可能在任何阶段触发，不覆盖正在做的事。
                let panel = &mut self.docs[index].ai_panel;
                if panel
                    .running_turn_mut()
                    .is_some_and(|turn| turn.phase.is_empty())
                {
                    panel.pi_progress("候选文件已更新");
                }
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::PiPrepared {
                turn_id,
                reference,
                dev_runtime,
                bridge,
                kind,
                ack,
            } => {
                let doc = &mut self.docs[index];
                // 只认仍属于本任务的等待轮次；过期事件直接丢掉 ack，后台有界等待会结束。
                let valid = doc.ai_panel.turns.iter().any(|turn| {
                    turn.id == turn_id
                        && matches!(turn.state, crate::ai_panel::TurnState::Waiting)
                        && turn.usage_job_seq == Some(seq)
                });
                if !valid {
                    return;
                }
                if doc.read_only()
                    || reference.baseline != crate::pi_agent::sha256_hex(&doc.generated_markdown)
                    || kind != doc.draft.kind.label()
                    || doc
                        .ai_panel
                        .cancel
                        .as_ref()
                        .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed))
                {
                    ack.confirm(Err(
                        "准备期间正文或文种已变化、稿件已锁定或任务已停止，请重新发送。".into(),
                    ));
                    return;
                }
                let previous_run = if let Some(turn) = doc.ai_panel.turn_mut(turn_id) {
                    let previous_run = turn.pi_run.replace(reference);
                    turn.phase = "正在启动 Pi host…".into();
                    if dev_runtime && !turn.context.iter().any(|line| line.contains("开发运行时"))
                    {
                        turn.context.push("开发运行时（未经安装包验证）".into());
                    }
                    previous_run
                } else {
                    None
                };
                // 成功绑定新运行之后才清旧提案；准备失败时旧提案仍可继续审阅。
                let previous_ask = doc.ai_panel.pi_ask.take();
                let previous_report = doc.ai_panel.pi_report.take();
                let previous_sources = doc.ai_panel.pi_sources.take();
                let previous_proposal = doc.ai_proposal.take();
                let previous_baseline = doc
                    .ai_review_baseline
                    .replace(doc.generated_markdown.clone());
                // 有库稿件必须先保存运行引用；保存不成功就不启动 host。
                let save_error = if let Some(manuscript_id) = doc.manuscript_id {
                    if self.manuscript_store.is_none() {
                        Some("稿件库没有打开，无法保存 Pi 运行引用；未启动 host。".to_string())
                    } else if doc.ai_panel.session.loaded_for != Some(manuscript_id) {
                        Some(
                            "这篇稿件还没有和稿件库对齐，无法保存 Pi 运行引用；请先保存稿件再试。"
                                .to_string(),
                        )
                    } else {
                        super::session::save_ai_session(doc, self.manuscript_store.as_mut())
                    }
                } else {
                    None
                };
                if let Some(error) = save_error {
                    doc.ai_panel.pi_ask = previous_ask;
                    doc.ai_panel.pi_report = previous_report;
                    doc.ai_panel.pi_sources = previous_sources;
                    doc.ai_proposal = previous_proposal;
                    doc.ai_review_baseline = previous_baseline;
                    // 运行记录虽然已写好，但没有落进会话就不给恢复入口，也不启动 host。
                    if let Some(turn) = doc.ai_panel.turn_mut(turn_id) {
                        turn.pi_run = previous_run;
                    }
                    self.status = error.clone();
                    ack.confirm(Err(error));
                    return;
                }
                doc.ai_panel.pi_bridge = Some(bridge);
                ack.confirm(Ok(()));
                self.status = "Pi 运行引用已保存，正在启动 host…".into();
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::PiDone(result) => self.apply_pi_done(index, &prefix, ctx, *result),
            #[cfg(feature = "ai-workbench")]
            DocJob::PiQuestions(batch) => {
                let bridge = self.docs[index].ai_panel.pi_bridge.clone();
                match bridge {
                    Some(bridge) => {
                        self.docs[index].ai_panel.pi_ask =
                            Some(crate::ai_panel::pi_ask::PiAsk::new(batch, bridge));
                        if let Some(turn) = self.docs[index].ai_panel.running_turn_mut() {
                            // Pi 等待回答期间进程仍活着，按 Streaming 保持「运行中」：
                            // 停止按钮、计时和新的并发保护都以它为准；题卡另有状态。
                            turn.state = TurnState::Streaming;
                        }
                        self.status = format!("{prefix}Pi 在等你回答必要问题。");
                    }
                    None => {
                        self.docs[index].ai_panel.finish(TurnState::Failed(
                            "Pi 提问时任务已结束，回答没有送回。".into(),
                        ));
                        self.docs[index].busy = false;
                    }
                }
            }
            // 用量按原轮次回投，这一支不该走到这里；真走到就忽略，不让任务崩掉。
            #[cfg(feature = "ai-workbench")]
            DocJob::AiUsage { .. } => {}
            #[cfg(feature = "ai-workbench")]
            DocJob::AiWorkspace(text) => self.docs[index].ai_panel.replace_content(text),
            #[cfg(feature = "ai-workbench")]
            DocJob::AiWriteBegin { prefix, suffix } => {
                self.docs[index].ai_panel.begin_write(prefix, suffix)
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::SkillDone(result) => self.apply_skill_done(index, &prefix, result),
            #[cfg(feature = "ai-workbench")]
            DocJob::AiSessionSummary { summary, upto } => {
                let session = &mut self.docs[index].ai_panel.session;
                session.summary = summary;
                session.compacted_upto = upto;
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::AiCompactDone(Ok((summary, upto, count))) => {
                let session = &mut self.docs[index].ai_panel.session;
                session.summary = summary;
                session.compacted_upto = upto;
                self.status =
                    format!("{prefix}会话已压缩：前面 {count} 轮并成了摘要，原始记录仍可翻看。");
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::AiCompactDone(Err(error)) => {
                self.status = format!("{prefix}压缩会话失败：{error}");
            }
            #[cfg(feature = "ai-workbench")]
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
            #[cfg(feature = "ai-workbench")]
            DocJob::StyleUsed(id) => {
                if let Ok(mut book) = crate::ai_services::style::StyleBook::load() {
                    book.record_use(&id);
                    let _ = book.save();
                }
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::AiNote(note) => {
                self.status = format!("{prefix}{note}");
                self.docs[index].ai_panel.note(note);
            }
            #[cfg(feature = "ai-workbench")]
            DocJob::AiFieldSuggestions(list) => {
                self.docs[index].ai_panel.offer_field_suggestions(list);
            }
            DocJob::ExportProgress(message) => {
                #[cfg(feature = "ai-workbench")]
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
                // 成品不再进抽屉：让工具栏的 PDF / Word / Typst 包入口重新扫盘点亮即可。
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
                // 公函两类随行单据出了几份，一并说出来：光看文件数不知道签收单与
                // 传发任务单是不是真的出来了。
                let sheets_tail = outcome.sheets.describe();
                // 孤行、编译失败、要素未齐都是这一下才摆到眼前的，弹抽屉把它们顶出来，
                // 别让提示躺在折叠面板里看不见。
                if orphans > 0 || mustfix > 0 || self.docs[index].export_error.is_some() {
                    self.docs[index].result_drawer_open = true;
                }
                self.status = if self.docs[index].export_error.is_some() {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件{sheets_tail}；PDF 编译失败，请查看审校提示。",
                        self.docs[index].output_files.len()
                    )
                } else if orphans > 0 {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件{sheets_tail}；实测发现 {orphans} 处排版问题，见审校提示{mustfix_tail}。",
                        self.docs[index].output_files.len()
                    )
                } else {
                    format!(
                        "{prefix}当前审校稿已导出 {} 个文件{sheets_tail}{mustfix_tail}。",
                        self.docs[index].output_files.len()
                    )
                };
                if let Some(path) = self.docs[index].output_files.first() {
                    self.status.push_str(&format!(
                        " 输出位置：{}",
                        path.parent().unwrap_or(path).display()
                    ));
                }
                // 起草页「打印…」：PDF 排好就打开它，打开完自动弹打印对话框。
                // 标记取一次就清；排版失败或没出 PDF 就不打印。
                if self.docs[index].take_print_after_export() {
                    let pdf = super::printing::exported_pdf_to_print(
                        &self.docs[index].output_files,
                        self.docs[index].export_error.as_deref(),
                    );
                    match pdf {
                        Some(path) => {
                            let hints = crate::print::PrintHints {
                                duplex_typeset: self.docs[index].draft.profile.duplex_printing,
                                send_package: false,
                            };
                            self.open_pdf_for_printing(path, hints);
                            self.status.push_str(" 正在打开打印对话框…");
                        }
                        None => self.status.push_str(" PDF 没有排出来，无法打印。"),
                    }
                }
                // 「输出」分区的成品图标：导完直接打开这份成品，不必再去翻目录。
                if let Some(kind) = self.docs[index].take_open_after_export() {
                    self.open_exported(index, kind);
                }
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
            DocJob::MemoryPrintPdf(result) => match result {
                Ok(bytes) => {
                    self.status = format!("{prefix}打印 PDF 已在内存中排版。");
                    self.open_pdf_memory(-(self.next_pdf_key as i64) - 1, "打印预览".into(), bytes);
                    if let Some(pdf) = self.pdfs.last_mut() {
                        pdf.request_print_when_ready_confirmed();
                    }
                }
                Err(error) => self.status = format!("{prefix}打印排版失败：{error}"),
            },
            DocJob::RedlinePrintPreview(Ok(bytes)) => {
                self.status = format!("{prefix}花脸稿打印预览已生成（仅在内存中）。");
                self.open_pdf_memory(
                    -(self.next_pdf_key as i64) - 1,
                    "花脸稿打印预览".into(),
                    bytes,
                );
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
                // 「打印…」等的那份 PDF 没出来：清掉标记，说明不能打印。
                if doc.take_print_after_export() {
                    self.status.push_str(" PDF 没有排出来，无法打印。");
                }
                doc.take_open_after_export();
            }
        }
    }

    /// 打开刚导出的一种成品：PDF 在应用内、Word 用系统程序、Typst 包在文件管理器里
    /// 定位。结果缀在导出状态后面。
    fn open_exported(&mut self, index: usize, kind: crate::draft_page::ExportKind) {
        use crate::draft_page::ExportKind;
        let Some(path) = kind.find_in(&self.docs[index].output_files).cloned() else {
            self.status
                .push_str(&format!(" 没有生成 {}，无法打开。", kind.label()));
            return;
        };
        let result = match kind {
            ExportKind::Pdf => {
                let hints = crate::print::PrintHints {
                    duplex_typeset: self.docs[index].draft.profile.duplex_printing,
                    send_package: false,
                };
                self.reopen_exported_pdf(path, hints);
                Ok("已在应用内打开。")
            }
            ExportKind::Word => {
                self.request_output_with_warning(
                    format!("用系统程序打开 Word：{}", path.display()),
                    super::output_guard::EXTERNAL_OPEN_WARNING,
                    move |app| match crate::app::open_in_os(&path) {
                        Ok(()) => app.status.push_str(" 已用系统程序打开。"),
                        Err(error) => app.status.push_str(&format!(" 打开失败：{error}")),
                    },
                );
                return;
            }
            ExportKind::Typst => crate::app::reveal_in_os(&path).map(|()| "已在文件管理器里定位。"),
        };
        match result {
            Ok(done) => self.status.push_str(&format!(" {done}")),
            Err(error) => self.status.push_str(&format!(" 打开失败：{error}")),
        }
    }
}

/// 结果摘要里的失败部分：「，N 篇失败（《甲》：原因；…）」，最多列三条；无失败为空串。
#[cfg(feature = "ai-workbench")]
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

#[cfg(test)]
#[cfg(feature = "ai-workbench")]
mod tests {
    use super::*;
    use crate::app::management_samples;

    /// 起一轮等待中的 Pi 任务，返回轮次 id 与本轮任务序号。
    fn waiting_pi_turn(app: &mut GongwenApp, seq: u64) -> u64 {
        let turn = app.docs[0].ai_panel.push_turn(
            "智能写作（Pi）".into(),
            "写一篇长材料".into(),
            vec!["Pi 文件工作区".into()],
            None,
        );
        let item = app.docs[0].ai_panel.turn_mut(turn).unwrap();
        item.state = TurnState::Waiting;
        item.usage_job_seq = Some(seq);
        app.docs[0].job_seq = seq;
        turn
    }

    #[test]
    fn pi_activity_reports_one_completed_step_and_preserves_stage() {
        let mut app = management_samples::fixture();
        let turn = waiting_pi_turn(&mut app, 7);
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            7,
            #[cfg(feature = "ai-workbench")]
            DocJob::PiProgress("正在写第 2/4 节「工作安排」".into()),
        );
        let activity = |outcome: Option<&str>| {
            #[cfg(feature = "ai-workbench")]
            DocJob::PiActivity(crate::pi_agent::PiActivity {
                id: "search".into(),
                action: "查知识库 · 整改时限".into(),
                outcome: outcome.map(str::to_owned),
            })
        };
        app.apply_doc_job(key, 7, activity(None));
        app.apply_doc_job(key, 7, DocJob::PiCandidate);
        {
            let item = app.docs[0].ai_panel.turn_mut(turn).unwrap();
            assert_eq!(item.pi_active.len(), 1);
            assert!(item.steps.is_empty());
            assert_eq!(item.phase, "正在写第 2/4 节「工作安排」");
        }
        app.apply_doc_job(key, 7, activity(Some("未找到相关资料")));
        app.apply_doc_job(key, 6, activity(Some("迟到结果")));
        let item = app.docs[0].ai_panel.turn_mut(turn).unwrap();
        assert!(item.pi_active.is_empty());
        assert_eq!(
            item.pi_recent.as_deref(),
            Some("未找到相关资料 · 查知识库 · 整改时限")
        );
        assert_eq!(item.steps, ["查知识库 · 整改时限 · 未找到相关资料"]);
    }

    /// Pi 候选只报进度：不再切到「AI 工作稿」，正文保持原样等交付。
    #[test]
    fn pi_candidate_only_reports_progress_without_opening_a_workspace() {
        let mut app = management_samples::fixture();
        let turn = waiting_pi_turn(&mut app, 7);
        let key = app.docs[0].key;
        app.apply_doc_job(key, 7, DocJob::PiCandidate);
        assert!(
            app.docs[0].ai_panel.workspace_view.is_none(),
            "Pi 候选不能再自动展开工作稿"
        );
        let item = app.docs[0]
            .ai_panel
            .turns
            .iter()
            .find(|item| item.id == turn)
            .unwrap();
        assert!(!item.is_workspace);
        assert!(item.content.is_empty(), "候选内容不再进侧栏工作稿");
        assert!(item.phase.contains("候选文件已更新"), "{}", item.phase);
    }
}
