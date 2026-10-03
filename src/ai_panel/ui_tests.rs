//! AI 侧栏界面的测试：发起、停止、选区、红线 1 与结果卡。

use super::*;
use crate::app::VersionSwitchPrompt;
use crate::app::WorkerResult;
use crate::draft_page::{DraftSession, ExportLinks};
use crate::models::{AppConfig, GeneratedDraft};
use std::sync::mpsc::{Receiver, Sender};

struct Harness {
    ctx: egui::Context,
    doc: DraftSession,
    config: AppConfig,
    sender: Sender<WorkerResult>,
    status: String,
    version_switch: Option<VersionSwitchPrompt>,
    revert_confirm: Option<(i64, i64)>,
    actions: Vec<DraftAction>,
    export_links: ExportLinks,
    metrics: crate::metrics::Metrics,
    _keep: Receiver<WorkerResult>,
}

impl Harness {
    fn new(markdown: &str) -> Self {
        // 发起任务会顺手存一次配置，不能写到用户真正的配置目录里去。
        let dir = std::env::temp_dir().join(format!(
            "gongwen-ai-panel-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        crate::storage::set_test_config_dir(Some(dir));
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let config = AppConfig::default();
        let doc = DraftSession::with_markdown(1, &config, markdown.to_string());
        let (sender, _keep) = std::sync::mpsc::channel();
        Self {
            ctx,
            doc,
            config,
            sender,
            status: String::new(),
            version_switch: None,
            revert_confirm: None,
            actions: Vec::new(),
            export_links: ExportLinks::default(),
            metrics: crate::metrics::Metrics::default(),
            _keep,
        }
    }

    fn with_page<R>(&mut self, f: impl FnOnce(&mut DraftPage<'_>) -> R) -> R {
        let mut page = DraftPage {
            doc: &mut self.doc,
            config: &mut self.config,
            store: None,
            sender: &self.sender,
            status: &mut self.status,
            version_switch: &mut self.version_switch,
            revert_confirm: &mut self.revert_confirm,
            actions: &mut self.actions,
            export_links: &mut self.export_links,
            metrics: &mut self.metrics,
        };
        f(&mut page)
    }

    /// 画一帧侧栏，返回画面上所有文字。
    fn frame_texts(&mut self) -> Vec<String> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            ..Default::default()
        };
        let ctx = self.ctx.clone();
        let output = ctx.run_ui(raw, |ui| self.with_page(|page| page.ai_panel_side(ui)));
        fn collect(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::epaint::Shape::Vec(shapes) => {
                    shapes.iter().for_each(|shape| collect(shape, out));
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }
}

fn has(texts: &[String], needle: &str) -> bool {
    texts.iter().any(|text| text.contains(needle))
}

#[test]
fn sending_opens_a_turn_and_stop_releases_the_document() {
    let mut harness = Harness::new("# 标题\n\n一、总体要求\n");
    harness.doc.ai_panel.open = true;
    harness.doc.ai_panel.composer.text = "压缩一下".into();
    harness.with_page(|page| page.send_ai_panel());

    let panel = &harness.doc.ai_panel;
    assert_eq!(panel.turns.len(), 1);
    assert_eq!(panel.turns[0].title, "润色");
    assert_eq!(panel.turns[0].prompt, "压缩一下");
    assert!(panel.running());
    assert!(panel.composer.text.is_empty(), "发出去后输入框清空");
    assert!(harness.doc.busy);
    let cancel = panel.cancel.clone().expect("应当留下停止开关");
    let seq = harness.doc.job_seq;

    harness.with_page(|page| page.stop_ai_task());
    assert!(cancel.load(Ordering::Relaxed), "停止要通知后台线程");
    assert!(!harness.doc.busy);
    assert_eq!(
        harness.doc.job_seq,
        seq + 1,
        "迟到的回投要因序号对不上被丢掉"
    );
    assert_eq!(harness.doc.ai_panel.turns[0].state, TurnState::Stopped);
}

#[test]
fn polish_needs_a_request_or_a_preset() {
    let mut harness = Harness::new("# 标题\n\n正文\n");
    harness.with_page(|page| page.send_ai_panel());
    assert!(harness.doc.ai_panel.turns.is_empty());
    assert!(!harness.doc.busy);
    assert!(harness.doc.ai_panel.composer.error.is_some());
}

#[test]
fn a_stale_selection_is_refused_instead_of_guessed() {
    let mut harness = Harness::new("# 标题\n\n二、乙\n\n二、乙\n");
    let composer = &mut harness.doc.ai_panel.composer;
    composer.text = "改一下".into();
    composer.selection = Some((0..3, "二、乙".into()));
    harness.with_page(|page| page.send_ai_panel());
    assert!(harness.doc.ai_panel.turns.is_empty());
    let error = harness.doc.ai_panel.composer.error.clone().unwrap();
    assert!(error.contains("选区"), "{error}");
}

#[test]
fn drafting_always_goes_through_review() {
    let mut harness = Harness::new("");
    let request = TurnRequest {
        mode: PanelMode::Draft,
        text: "根据材料写一份通知".into(),
        selection: None,
        preset: None,
        use_rag: true,
    };
    let (task, title, context) = harness.with_page(|page| page.panel_task(&request)).unwrap();
    // 红线 1：空稿起草也要用户点一次才落入正文。
    assert!(task.review_before_apply);
    // 知识库没启用时不检索，也不挂「知识库」chip。
    assert!(!task.use_rag);
    assert_eq!(title, "起草");
    assert!(context.is_empty());
    assert!(task.material.contains("根据材料写一份通知"));
}

#[test]
fn toggling_with_a_selection_locks_it_for_polish() {
    let mut harness = Harness::new("# 标题\n\n一、总体要求\n");
    harness.doc.result_drawer_open = true;
    let start = harness.doc.generated_markdown.find("一、").unwrap();
    harness.with_page(|page| page.toggle_ai_panel(Some(start..start + "一、".len())));
    let panel = &harness.doc.ai_panel;
    assert!(panel.open);
    assert!(!harness.doc.result_drawer_open, "与审校抽屉互斥");
    assert_eq!(panel.composer.mode, PanelMode::Polish);
    assert_eq!(panel.composer.selection.as_ref().unwrap().1, "一、");
    // 不带选区再点一下就收起。
    harness.with_page(|page| page.toggle_ai_panel(None));
    assert!(!harness.doc.ai_panel.open);
}

#[test]
fn empty_document_opens_in_draft_mode() {
    let mut harness = Harness::new("");
    harness.with_page(|page| page.toggle_ai_panel(None));
    assert_eq!(harness.doc.ai_panel.composer.mode, PanelMode::Draft);
}

#[test]
fn knowledge_search_defaults_to_the_global_switch_only_once() {
    let mut harness = Harness::new("");
    harness.config.rag.enabled = true;
    harness.with_page(|page| page.toggle_ai_panel(None));
    assert!(
        harness.doc.ai_panel.composer.use_rag,
        "启用了知识库，起草默认检索"
    );
    // 用户取消勾选后，再开关侧栏也不被改回去。
    harness.doc.ai_panel.composer.use_rag = false;
    harness.with_page(|page| page.toggle_ai_panel(None));
    harness.with_page(|page| page.toggle_ai_panel(None));
    assert!(!harness.doc.ai_panel.composer.use_rag);
}

#[test]
fn knowledge_notes_stay_on_the_card() {
    let mut harness = Harness::new(
        "# 标题
",
    );
    harness.doc.ai_panel.open = true;
    harness
        .doc
        .ai_panel
        .push_turn("起草 · 知识库".into(), "写个报告".into(), vec![], None);
    harness
        .doc
        .ai_panel
        .note("知识库：已注入 5 段知识库参考：《梅文项目》".into());
    harness.doc.ai_panel.append("一、背景", "", false);
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(
        has(&texts, "已注入 5 段知识库参考：《梅文项目》"),
        "{texts:?}"
    );
}

#[test]
fn result_card_offers_adoption_and_accepting_writes_the_body() {
    let mut harness = Harness::new("");
    harness.doc.ai_panel.open = true;
    harness
        .doc
        .ai_panel
        .push_turn("起草".into(), "写个通知".into(), vec![], None);
    harness
        .doc
        .ai_panel
        .append("# 关于冬季防火的通知", "", true);
    harness.doc.ai_proposal = Some(AiProposal {
        before: String::new(),
        result: GeneratedDraft {
            markdown: "# 关于冬季防火的通知\n".into(),
            title: "关于冬季防火的通知".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "起草".into(),
        fact_changes: Vec::new(),
        fact_changes_confirmed: false,
        view: Default::default(),
        open: false,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary {
            chars: 11,
            was_empty: true,
            ..Default::default()
        }));
    // 侧栏要先布一帧局，第二帧才是稳定画面。
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "采用为正文"), "{texts:?}");
    assert!(has(&texts, "新稿 11 字"), "{texts:?}");
    // 空稿没有可对照的旧文，不给「查看对照」。
    assert!(!has(&texts, "查看对照"), "{texts:?}");
    // 结果出来之前，正文一个字都没动。
    assert!(harness.doc.generated_markdown.is_empty());

    assert!(harness.with_page(|page| GongwenApp::accept_ai_proposal(page.doc, page.status)));
    assert_eq!(harness.doc.generated_markdown, "# 关于冬季防火的通知\n");
    assert_eq!(harness.doc.ai_panel.turns[0].state, TurnState::Accepted);
}

#[test]
fn unconfirmed_fact_changes_block_acceptance() {
    let mut harness = Harness::new("# 标题\n\n2025年完成。\n");
    let before = harness.doc.generated_markdown.clone();
    let after = "# 标题\n\n2026年完成。\n".to_string();
    let fact_changes =
        crate::ai_guard::compare_key_facts(&before, &after, &harness.config.vocabulary);
    assert!(!fact_changes.is_empty(), "年份变化应当被认作关键事实变化");
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "改年份".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: before.clone(),
        result: GeneratedDraft {
            markdown: after.clone(),
            title: "标题".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes,
        fact_changes_confirmed: false,
        view: Default::default(),
        open: false,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));

    assert!(!harness.with_page(|page| GongwenApp::accept_ai_proposal(page.doc, page.status)));
    assert_eq!(harness.doc.generated_markdown, before, "没勾确认不能落地");
    assert!(harness.doc.ai_proposal.is_some(), "提案原样留着");

    harness
        .doc
        .ai_proposal
        .as_mut()
        .unwrap()
        .fact_changes_confirmed = true;
    assert!(harness.with_page(|page| GongwenApp::accept_ai_proposal(page.doc, page.status)));
    assert_eq!(harness.doc.generated_markdown, after);
}

#[test]
fn streaming_card_says_thinking_before_the_first_word() {
    let mut harness = Harness::new("# 标题\n");
    harness.doc.ai_panel.open = true;
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "压一压".into(), vec![], None);
    harness.doc.ai_panel.append("", "先看看结构", false);
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "正在思考"), "{texts:?}");
    assert!(has(&texts, "思考过程（5 字）"), "{texts:?}");
    assert!(has(&texts, "停止"), "{texts:?}");
}

/// 连真实模型把整条链路跑一遍：侧栏发出 → 后台流式攒批 → 闸门 → 提案。默认忽略；用法同
/// `lmstudio::stream` 的 `live_model_stream`（设 `GONGWEN_LIVE_LLM_URL` 与 `GONGWEN_LIVE_LLM_MODEL`）。
#[test]
#[ignore = "需要本机模型服务"]
fn live_panel_round_trip() {
    let (Ok(url), Ok(model)) = (
        std::env::var("GONGWEN_LIVE_LLM_URL"),
        std::env::var("GONGWEN_LIVE_LLM_MODEL"),
    ) else {
        eprintln!("未设置 GONGWEN_LIVE_LLM_URL / GONGWEN_LIVE_LLM_MODEL，跳过");
        return;
    };
    let mut harness = Harness::new("");
    harness.config.lm_studio.base_url = url;
    harness.config.lm_studio.model = model;
    harness.config.lm_studio.max_tokens = 6000;
    harness.doc.ai_panel.composer.mode = PanelMode::Draft;
    harness.doc.ai_panel.composer.text =
        "根据以下要点起草一份通知：各区县做好冬季森林防火；12月1日前完成隐患排查；落实24小时值班。"
            .into();
    harness.with_page(|page| page.send_ai_panel());
    assert!(harness.doc.busy);

    let started = std::time::Instant::now();
    let mut batches = 0usize;
    let mut done_seen = false;
    loop {
        let message = harness
            ._keep
            .recv_timeout(Duration::from_secs(300))
            .expect("五分钟内应当有结果");
        let WorkerResult::Doc { job, .. } = message else {
            continue;
        };
        match job {
            crate::app::DocJob::AiStream {
                content,
                reasoning,
                done,
            } => {
                batches += 1;
                done_seen |= done;
                harness.doc.ai_panel.append(&content, &reasoning, done);
            }
            crate::app::DocJob::ExportProgress(message) => {
                harness.doc.ai_panel.set_phase(&message);
            }
            crate::app::DocJob::Proposed(result) => {
                let result = result.expect("提案应当生成成功");
                let turn = &harness.doc.ai_panel.turns[0];
                eprintln!(
                    "攒批 {batches} 次；思考 {} 字；流式正文 {} 字；提案 {} 字；审校提示 {} 条；耗时 {:?}\n{}",
                    turn.reasoning.chars().count(),
                    turn.content.chars().count(),
                    result.markdown.chars().count(),
                    result.warnings.len(),
                    started.elapsed(),
                    result.markdown
                );
                assert!(done_seen, "最后一批应当带 done");
                assert!(batches > 2, "应当是分批到达的");
                assert_eq!(turn.state, TurnState::Checking);
                assert!(!result.markdown.trim().is_empty());
                // 提案只是提案：正文一个字都没动。
                assert!(harness.doc.generated_markdown.is_empty());
                break;
            }
            other => panic!("意外的回投：{}", std::any::type_name_of_val(&other)),
        }
    }
}
