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
use crate::file_clipboard;
use crate::images;
use crate::lmstudio;
use crate::models::{DraftInput, ExportSelection, GeneratedDraft, ReviewNote, TemplateKind};
use crate::prompt;
use crate::rag;
use crate::revise_model;
use crate::storage;
use crate::theme;
use crate::validator;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

/// 流式增量最多攒这么久就发一次。
const STREAM_FLUSH_INTERVAL: Duration = Duration::from_millis(50);
/// 或者攒满这么多字节（约 200 个汉字）就发。两条取先到者：既不让界面一卡一卡，
/// 也不至于一个 token 一条消息、长稿几千次重绘。
const STREAM_FLUSH_BYTES: usize = 600;

/// 流式调用起草模型，增量攒批投回界面。最后一批带 `done`，界面据此切到「校验中」。
fn stream_draft_model(
    config: &crate::models::LmStudioConfig,
    system: &str,
    user: &str,
    cancel: &AtomicBool,
    tx: &Sender<WorkerResult>,
    key: DocKey,
    seq: u64,
) -> anyhow::Result<lmstudio::StreamOutcome> {
    let send = |content: String, reasoning: String, done: bool| {
        let _ = tx.send(WorkerResult::Doc {
            key,
            seq,
            job: DocJob::AiStream {
                content,
                reasoning,
                done,
            },
        });
    };
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut last_flush = Instant::now();
    let outcome = lmstudio::generate_stream(
        config,
        system,
        user,
        config.temperature,
        config.max_tokens,
        lmstudio::ChatOptions::default(),
        cancel,
        |delta| {
            match delta {
                lmstudio::StreamDelta::Content(text) => content.push_str(text),
                lmstudio::StreamDelta::Reasoning(text) => reasoning.push_str(text),
            }
            if last_flush.elapsed() >= STREAM_FLUSH_INTERVAL
                || content.len() + reasoning.len() >= STREAM_FLUSH_BYTES
            {
                send(
                    std::mem::take(&mut content),
                    std::mem::take(&mut reasoning),
                    false,
                );
                last_flush = Instant::now();
            }
        },
    );
    let finished = matches!(&outcome, Ok(o) if o.finish != lmstudio::Finish::Cancelled);
    if finished || !content.is_empty() || !reasoning.is_empty() {
        send(content, reasoning, finished);
    }
    outcome
}

/// 起草时检索知识库并把结果拼成提示词参考节。检索失败降级为空串，不阻塞
/// 起草——RAG 是增强而非硬依赖；但降级原因会随 `notes` 回给调用方显示，
/// 不再是只有翻服务端日志才知道的静默失败。
///
/// 检索本身与知识库页的检索、问答走同一个 [`rag::retrieve`]，检索词也同样是
/// 用户自己写的那段话（见 [`retrieval_query`]）。
///
/// 返回 (参考节, 给用户看的说明)。
pub(crate) fn retrieve_reference(
    rag_cfg: &crate::models::RagConfig,
    chat: &crate::models::LmStudioConfig,
    input: &DraftInput,
    query: &str,
    kind_filter: Option<TemplateKind>,
) -> (String, Vec<String>) {
    let query = retrieval_query(input, query);
    if query.is_empty() {
        return (
            String::new(),
            vec!["没有可用的检索词（材料和标题提示都是空的），本次未检索知识库。".into()],
        );
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
        notes.push(match kind_filter {
            Some(kind) => format!(
                "知识库里没有检索到「{}」文种的相关片段，本次未注入参考；可把检索范围改为「全部文种」。",
                kind.label()
            ),
            None => "知识库没有检索到相关片段，本次未注入参考。".into(),
        });
        return (String::new(), notes);
    }
    notes.push(reference_note(&outcome.chunks));
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
    (prompt::format_reference_section(&refs), notes)
}

/// 知识库检索词：用户自己写的那段话；为空时退回标题提示。
///
/// 与知识库页的问答一致，只拿用户的原话去搜，不拼提示词外壳，也不在前面硬加
/// 标题提示——原先拼的是整段起草材料（含「【已确认事实单——优先于原始材料】」
/// 「材料要点 1」这类标签），再截成 300 字，标签字挤掉了真正的内容。
fn retrieval_query(input: &DraftInput, query: &str) -> String {
    let query = query.trim();
    if query.is_empty() {
        input.title_hint.trim().to_string()
    } else {
        query.to_string()
    }
}

/// 「已注入 N 段参考：《甲》《乙》」。点名出处，用户才知道模型照着什么写的。
fn reference_note(chunks: &[rag::RetrievedChunk]) -> String {
    let mut titles: Vec<&str> = Vec::new();
    for chunk in chunks {
        if !titles.contains(&chunk.doc_title.as_str()) {
            titles.push(&chunk.doc_title);
        }
    }
    let named = titles
        .iter()
        .map(|title| format!("《{title}》"))
        .collect::<String>();
    format!("已注入 {} 段知识库参考：{named}", chunks.len())
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
            *self.status = "文字复核未启用：请先在设置中配置复核用的小模型。".into();
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
            *self.status = "没有启用任何检查器：请在「设置 → AI 文字复核」中至少开启一项。".into();
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
        let draft_model = self.config.lm_studio.clone();
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
                    draft_model: &draft_model,
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

    /// 让模型先列一份章节大纲。只出骨架，不写正文。
    pub(crate) fn start_outline(&mut self, material: String, query: String, use_rag: bool) {
        if self.doc.busy {
            return;
        }
        let (key, seq) = self.begin_job();
        *self.status = "正在列大纲…".into();
        self.doc.outline = Some(crate::draft_page::OutlineDraft {
            outline: crate::outline::Outline::default(),
            material: material.clone(),
            running: None,
            open: true,
            error: None,
        });

        let time_context = prompt::TimeContext::now();
        let input = self.doc.draft.clone();
        let config = self.config.clone();
        let rag_kind = self.doc.rag_kind_filter.resolve(self.doc.draft.kind);
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = (|| {
                let reference = if use_rag && config.rag.enabled {
                    retrieve_reference(&config.rag, &config.lm_studio, &input, &query, rag_kind).0
                } else {
                    String::new()
                };
                let system = prompt::build_system_prompt(&time_context);
                let user = crate::outline::build_outline_prompt(
                    &input,
                    &config.vocabulary,
                    &material,
                    &reference,
                );
                let raw = lmstudio::generate(&config.lm_studio, &system, &user)?;
                Ok::<_, anyhow::Error>(crate::outline::parse_outline_reply(&raw))
            })()
            .map_err(|error: anyhow::Error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::Outlined(result),
            });
        });
    }

    /// 按已确认的大纲生成其中一节。
    ///
    /// 一次只跑一节：这样某一节写坏了只重跑那一节，前面写好的不受影响——
    /// 这正是拆成大纲流程要换的东西。
    pub(crate) fn start_section_draft(&mut self, section: usize) {
        if self.doc.busy {
            return;
        }
        let Some(draft) = self.doc.outline.as_ref() else {
            return;
        };
        let Some(user) = crate::outline::build_section_prompt(
            &self.doc.draft,
            &self.config.vocabulary,
            &draft.outline,
            section,
            &draft.material,
        ) else {
            return;
        };
        let heading = draft.outline.sections[section].heading.clone();
        let (key, seq) = self.begin_job();
        *self.status = format!("正在生成第 {} 节「{heading}」…", section + 1);
        if let Some(draft) = self.doc.outline.as_mut() {
            draft.running = Some(section);
            draft.error = None;
            if let Some(item) = draft.outline.sections.get_mut(section) {
                item.state = crate::outline::SectionState::Running;
            }
        }

        let time_context = prompt::TimeContext::now();
        let config = self.config.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let system = prompt::build_system_prompt(&time_context);
            let result = lmstudio::generate(&config.lm_studio, &system, &user)
                .map(|raw| prompt::sanitize_model_markdown(&raw))
                .map_err(|error: anyhow::Error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::SectionDrafted {
                    index: section,
                    result,
                },
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
            query: instruction.clone(),
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
        // 侧栏发起的已经开好了这一轮；旧工作台发起的在这里补一轮，流式输出一样
        // 落进侧栏，用户随时能看、能停。
        if !self.doc.ai_panel.has_waiting_turn() {
            let context = if request.use_rag {
                vec!["知识库".to_string()]
            } else {
                Vec::new()
            };
            self.doc.ai_panel.push_turn(
                request.label.clone(),
                request.label.clone(),
                context,
                None,
            );
        }
        self.doc.ai_panel.open = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.doc.ai_panel.cancel = Some(cancel.clone());
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
        let query = request.query;
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
                            &query,
                            rag_kind,
                        );
                        // 检索结果与降级说明要让用户看见：一条条留在侧栏卡片上，
                        // 不能像阶段提示那样第一个字一到就被冲掉。
                        for note in notes {
                            let _ = tx.send(WorkerResult::Doc {
                                key,
                                seq,
                                job: DocJob::AiNote(format!("知识库：{note}")),
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
                let outcome =
                    stream_draft_model(&config.lm_studio, &system, &user, &cancel, &tx, key, seq)?;
                if outcome.finish == lmstudio::Finish::Cancelled {
                    // 界面在点停止时已经推进了任务序号，这条结果回去也会被丢掉。
                    anyhow::bail!("已停止生成");
                }
                let truncated = outcome.finish == lmstudio::Finish::Length;
                let raw = outcome.content;
                let cleaned = prompt::sanitize_model_markdown(&raw);
                let normalized = prompt::normalize_generated_markdown(&input, &cleaned);
                let markdown = export::finalize_markdown(&input, &normalized);
                let title = export::document_title(&input, &markdown);
                let mut warnings: Vec<ReviewNote> = validator::validate(
                    &input,
                    &markdown,
                    &config.vocabulary,
                    &config.security_rules,
                )
                .into_iter()
                .map(ReviewNote::from)
                .chain(validator::research_anchor_notes(&input, &markdown))
                .chain(validator::research_citation_notes(&input, &markdown))
                .collect();
                let mut proof_warnings: Vec<ReviewNote> = Vec::new();
                let mut proof_measured = false;
                let estimated = validator::estimate_layout_notes(&markdown);
                // 同上：要素没齐也照导，只有做不出成品的情况才跳过导出这一步。
                let blockers = validator::compile_blocking_issues(
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
                        &config.numbering,
                        None,
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
                                .map(|message| ReviewNote::from(format!("无法出稿：{message}"))),
                        );
                    }
                    vec![]
                };
                if !proof_measured {
                    warnings.extend(estimated);
                }
                if truncated {
                    warnings.push(ReviewNote::from(format!(
                        "{}可在设置里调大「最大输出」后重新生成。",
                        crate::ai_panel::TRUNCATED_NOTE
                    )));
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

#[cfg(test)]
mod retrieval_tests {
    use super::*;
    use crate::draft_page::RagKindFilter;

    fn chunk(title: &str) -> rag::RetrievedChunk {
        rag::RetrievedChunk {
            chunk_id: 0,
            doc_id: 0,
            doc_title: title.into(),
            kind: TemplateKind::ResearchReport,
            section: String::new(),
            text: String::new(),
            vector_score: 0.0,
            bm25_score: 0.0,
            fused_score: 0.0,
            rerank_score: None,
        }
    }

    #[test]
    fn the_query_is_the_users_own_words() {
        let input = DraftInput {
            title_hint: "冬季森林防火".into(),
            ..Default::default()
        };
        assert_eq!(retrieval_query(&input, "  隐患排查要点  "), "隐患排查要点");
        // 没写材料时才退回标题提示。
        assert_eq!(retrieval_query(&input, "  "), "冬季森林防火");
    }

    #[test]
    fn drafting_searches_all_kinds_by_default_like_the_knowledge_page() {
        assert_eq!(
            RagKindFilter::default().resolve(TemplateKind::OfficialLetter),
            None
        );
    }

    #[test]
    fn the_note_names_each_source_once() {
        let chunks = [chunk("甲"), chunk("乙"), chunk("甲")];
        assert_eq!(
            reference_note(&chunks),
            "已注入 3 段知识库参考：《甲》《乙》"
        );
    }

    /// 拿真实知识库对比改前改后的检索。默认忽略；需要：
    /// - `GONGWEN_LIVE_KB_DIR`：放着 `manuscripts.db` **副本**的目录；
    /// - `GONGWEN_LIVE_EMBED_URL` / `GONGWEN_LIVE_EMBED_MODEL`：与入库时同一个 embedding 模型。
    ///
    /// 不做重排，免得动用付费接口。
    #[test]
    #[ignore = "需要真实知识库与 embedding 服务"]
    fn live_knowledge_retrieval_before_and_after() {
        let (Ok(dir), Ok(url), Ok(model)) = (
            std::env::var("GONGWEN_LIVE_KB_DIR"),
            std::env::var("GONGWEN_LIVE_EMBED_URL"),
            std::env::var("GONGWEN_LIVE_EMBED_MODEL"),
        ) else {
            eprintln!(
                "未设置 GONGWEN_LIVE_KB_DIR / GONGWEN_LIVE_EMBED_URL / GONGWEN_LIVE_EMBED_MODEL，跳过"
            );
            return;
        };
        crate::storage::set_test_config_dir(Some(std::path::PathBuf::from(dir)));
        let mut rag_cfg = crate::models::RagConfig {
            enabled: true,
            ..Default::default()
        };
        rag_cfg.embedding.base_url = url;
        rag_cfg.embedding.model = model;
        rag_cfg.rerank.mode = crate::models::RerankMode::None;
        let chat = crate::models::LmStudioConfig::default();
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };

        let request = "根据知识库，起草一份关于人工智能辅助军事目标识别项目进展情况的报告";
        // 改前：检索词是整段起草材料（带提示词外壳），文种跟随当前稿件（公函）。
        let wrapped = format!(
            "【已确认事实单——优先于原始材料】\n- 材料要点 1：{request}\n\n【原始材料与写作要求】\n{request}"
        );
        let old_query = format!("{}\n{}", input.title_hint, wrapped);
        let (before, before_notes) = retrieve_reference(
            &rag_cfg,
            &chat,
            &input,
            &old_query,
            RagKindFilter::Follow.resolve(input.kind),
        );
        // 改后：检索词是用户原话，不限文种。
        let (after, after_notes) = retrieve_reference(
            &rag_cfg,
            &chat,
            &input,
            request,
            RagKindFilter::default().resolve(input.kind),
        );
        eprintln!(
            "改前：{before_notes:?}\n改后：{after_notes:?}\n改后参考节前 300 字：{}",
            after.chars().take(300).collect::<String>()
        );
        assert!(before.is_empty(), "跟随公函文种时应当检索不到研究报告");
        assert!(!after.is_empty(), "不限文种时应当命中");
    }
}
