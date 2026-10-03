//! AI 侧栏界面的测试：发起、停止、选区、红线 1 与结果卡。

use super::*;
use crate::agent::skill::RESEARCH_DRAFT;
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
        self.frame_with(Vec::new())
    }

    /// 把焦点放进侧栏输入框（下一帧生效）。
    fn focus_input(&mut self) {
        let id = egui::Id::new(("ai_panel_input", self.doc.key));
        self.ctx.memory_mut(|memory| memory.request_focus(id));
    }

    /// 带着按键画一帧。
    fn press(&mut self, key: egui::Key) -> Vec<String> {
        self.frame_with(vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }])
    }

    fn frame_output(&mut self, events: Vec<egui::Event>, size: egui::Vec2) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let ctx = self.ctx.clone();
        ctx.run_ui(raw, |ui| self.with_page(|page| page.ai_panel_side(ui)))
    }

    fn frame_with(&mut self, events: Vec<egui::Event>) -> Vec<String> {
        let output = self.frame_output(events, egui::vec2(1200.0, 900.0));
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
    assert_eq!(panel.turns[0].title, "精简扩写");
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
fn drafting_starts_research_and_only_ever_proposes() {
    let mut harness = Harness::new("");
    let composer = &mut harness.doc.ai_panel.composer;
    composer.text = "写一份冬季森林防火的通知".into();
    composer.use_rag = true;
    harness.with_page(|page| page.send_ai_panel());

    let panel = &harness.doc.ai_panel;
    assert_eq!(panel.turns.len(), 1);
    // 空稿只有起草类技能可用；知识库没启用时不检索，也不挂「知识库」chip。
    assert_eq!(panel.turns[0].title, "研究式起草");
    assert!(panel.turns[0].context.is_empty());
    assert!(panel.running());
    assert!(panel.cancel.is_some());
    assert!(harness.doc.busy);
    // 红线 1：结果只会装成提案，基线记下的是发起时的正文。
    assert_eq!(harness.doc.ai_review_baseline.as_deref(), Some(""));
    assert!(harness.doc.ai_proposal.is_none());
    assert!(harness.doc.generated_markdown.is_empty());
}

#[test]
fn predraft_answers_switch_the_kind_by_the_users_hand_and_continue() {
    use crate::agent::clarify::predraft_questions;
    use crate::models::TemplateKind;
    let mut harness = Harness::new("");
    harness.doc.draft.kind = TemplateKind::PlainDocument;
    harness.doc.ai_panel.open = true;
    let request = crate::ai_panel::TurnRequest {
        skill: Some(RESEARCH_DRAFT.into()),
        text: "起草一份商洽函".into(),
        selection: None,
        preset: None,
        use_rag: false,
        refs: Vec::new(),
        notes: Vec::new(),
    };
    let model_questions = vec![("篇幅多长？".to_string(), vec!["短".into(), "长".into()])];
    let questions = predraft_questions(
        &request.text,
        TemplateKind::PlainDocument,
        model_questions,
        3,
    );
    let board = crate::agent::board::Board {
        draft: harness.doc.draft.clone(),
        request: request.text.clone(),
        ..Default::default()
    };
    let panel = &mut harness.doc.ai_panel;
    let id = panel.push_turn(
        "研究式起草".into(),
        request.text.clone(),
        vec![],
        Some(request),
    );
    panel.ask(Box::new(crate::ai_panel::SkillRun {
        skill: crate::agent::skill::builtin(RESEARCH_DRAFT).unwrap(),
        board,
        suspension: crate::agent::engine::Suspension {
            questions,
            resume_at: 1,
            save_as: None,
        },
        use_rag: false,
    }));
    let turn = panel.turn_mut(id).unwrap();
    // 第二题选「长」；第一题沿用替用户选好的推荐项「切换为公函」。
    turn.replies[1].choice = Some(1);

    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "动笔前先确认这几件事"), "{texts:?}");
    assert!(has(&texts, "切换为公函（推荐）"), "{texts:?}");
    assert!(has(&texts, "确认，开始起草"), "{texts:?}");

    harness.with_page(|page| page.answer_suspended(id));
    assert_eq!(harness.doc.draft.kind, TemplateKind::OfficialLetter);
    let turn = harness.doc.ai_panel.turn_mut(id).unwrap();
    assert!(turn.state.running(), "答完接着跑，同一轮");
    assert!(turn.questions.is_empty());
    assert!(turn.run.is_none(), "挂起的流程已带回后台");
    assert_eq!(turn.request.as_ref().unwrap().notes, ["篇幅多长？长"]);
    assert_eq!(harness.doc.ai_panel.turns.len(), 1, "不另开一轮");
    assert!(harness.doc.busy);
}

#[test]
fn research_answers_become_a_new_proposal_without_calling_a_model() {
    use crate::agent::clarify::gap_questions;
    use crate::agent::gaps::{GapStatus, Ledger};
    let mut harness = Harness::new("");
    let raw = "# 关于冬季防火的通知\n\n请于【待核实：排查完成时限】前完成排查。\n".to_string();
    let mut ledger = Ledger::default();
    ledger.sync(&raw, "", &[]);
    let questions = gap_questions(&ledger, 4);
    assert_eq!(questions.len(), 1);
    harness.doc.ai_panel.open = true;
    let panel = &mut harness.doc.ai_panel;
    let id = panel.push_turn("起草".into(), "写个通知".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: String::new(),
        result: GeneratedDraft {
            markdown: raw.clone(),
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
    let panel = &mut harness.doc.ai_panel;
    panel.finish(TurnState::Proposed(ProposalSummary::default()));
    let turn = panel.turn_mut(id).unwrap();
    turn.replies = crate::ai_panel::initial_replies(&questions);
    turn.replies[0].custom = "12月1日".into();
    turn.questions = questions;
    turn.research = Some(crate::ai_panel::ResearchSnapshot {
        raw,
        ledger,
        sources: Vec::new(),
    });

    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "这几处要你确认"), "{texts:?}");
    assert!(has(&texts, "按回答修订"), "{texts:?}");

    harness.with_page(|page| page.apply_research_answers(id));
    let panel = &harness.doc.ai_panel;
    assert_eq!(panel.turns.len(), 2);
    assert_eq!(panel.turns[0].state, TurnState::Superseded);
    let new_turn = &panel.turns[1];
    assert!(matches!(new_turn.state, TurnState::Proposed(_)));
    assert!(new_turn.prompt.contains("12月1日"), "{}", new_turn.prompt);
    assert!(new_turn.questions.is_empty(), "答过的不再问");
    let research = new_turn.research.as_ref().unwrap();
    assert_eq!(
        research.ledger.gaps[0].status,
        GapStatus::Answered("12月1日".into())
    );
    let proposal = harness.doc.ai_proposal.as_ref().unwrap();
    assert!(
        proposal.result.markdown.contains("请于12月1日前完成排查"),
        "{}",
        proposal.result.markdown
    );
    // 仍然只是提案：正文没动，基线还是发起时的空稿。
    assert!(harness.doc.generated_markdown.is_empty());
    assert_eq!(proposal.before, "");
    assert!(!harness.doc.busy, "确定性替换，不起后台任务");
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
    assert_eq!(panel.composer.skill.as_deref(), Some(POLISH));
    assert_eq!(panel.composer.selection.as_ref().unwrap().1, "一、");
    // 不带选区再点一下就收起。
    harness.with_page(|page| page.toggle_ai_panel(None));
    assert!(!harness.doc.ai_panel.open);
}

#[test]
fn the_skill_chip_follows_the_document_and_the_input() {
    let mut harness = Harness::new("");
    harness.with_page(|page| page.toggle_ai_panel(None));
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(
        has(&texts, "输入 / 选技能"),
        "空稿、没写要求时几个起草类技能都可用：{texts:?}"
    );
    assert!(
        !has(&texts, "不用预设"),
        "空稿上润色不参与，没有预设下拉框：{texts:?}"
    );

    harness.doc.ai_panel.composer.text = "起草一份冬季森林防火的通知".into();
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "自动 · 研究式起草"), "{texts:?}");
    assert!(has(&texts, "检索知识库"), "研究式起草用知识库：{texts:?}");

    harness.doc.ai_panel.composer.text = "根据下面的会议纪要整理".into();
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "自动 · 材料成文"), "{texts:?}");
    assert!(!has(&texts, "检索知识库"), "材料成文不检索：{texts:?}");
    harness.doc.ai_panel.composer.text.clear();

    harness.doc.generated_markdown = "# 标题\n\n一、总体要求\n".into();
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "自动"), "{texts:?}");
    assert!(
        has(&texts, "输入 / 选技能"),
        "有稿、没写要求时给通用提示：{texts:?}"
    );

    harness.doc.ai_panel.composer.text = "压缩第二部分".into();
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "自动 · 精简扩写"), "{texts:?}");
    assert!(has(&texts, "不用预设"), "润色带预设下拉框：{texts:?}");
    assert!(has(&texts, "全文"), "{texts:?}");

    harness.doc.ai_panel.composer.skill = Some(RESEARCH_DRAFT.into());
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(
        has(&texts, "研究式起草"),
        "指定的技能优先于触发词：{texts:?}"
    );
    assert!(!has(&texts, "自动 · "), "{texts:?}");
}

#[test]
fn a_slash_prefix_offers_and_pins_skills() {
    let mut harness = Harness::new("# 标题\n\n正文\n");
    harness.doc.ai_panel.open = true;
    harness.doc.ai_panel.composer.text = "/润".into();
    harness.focus_input();
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "润色"), "列出匹配的技能：{texts:?}");
    assert!(has(&texts, "↑↓ 选择"), "光标处弹出技能列表：{texts:?}");
    assert!(!has(&texts, "仿写"), "随输入过滤：{texts:?}");

    // 回车选中高亮项：`/润` 从文字里去掉，技能标签换成润色。
    harness.press(egui::Key::Enter);
    let composer = &harness.doc.ai_panel.composer;
    assert_eq!(composer.skill.as_deref(), Some(POLISH));
    assert!(composer.text.is_empty(), "{:?}", composer.text);
    let texts = harness.frame_texts();
    assert!(!has(&texts, "↑↓ 选择"), "选完弹出层关掉：{texts:?}");
    harness.doc.ai_panel.composer.skill = None;

    harness.doc.ai_panel.composer.text = "/润色 只改错别字".into();
    harness.with_page(|page| page.send_ai_panel());
    let turn = &harness.doc.ai_panel.turns[0];
    assert_eq!(turn.title, "润色");
    assert!(harness.doc.busy);

    harness.with_page(|page| page.stop_ai_task());
    harness.doc.generated_markdown.clear();
    harness.doc.ai_panel.composer.text = "/润色 只改错别字".into();
    harness.with_page(|page| page.send_ai_panel());
    let error = harness.doc.ai_panel.composer.error.clone().unwrap();
    assert!(
        error.contains("正文还是空的"),
        "指定了用不了的技能要说明原因：{error}"
    );
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
fn a_report_card_lists_findings_by_group_and_points_to_the_drawer() {
    use crate::agent::board::{Finding, Fix};
    let mut harness = Harness::new(
        "# 标题

各地要加强巡查力度不断提高。
",
    );
    harness.doc.ai_panel.open = true;
    harness
        .doc
        .ai_panel
        .push_turn("全面审校".into(), "签发前审一下".into(), vec![], None);
    let finding = |group: &str, text: &str, fix: Option<Fix>| Finding {
        group: group.into(),
        text: text.into(),
        excerpt: "加强巡查力度不断提高".into(),
        source: "模型诊断，需人工判断".into(),
        fix,
    };
    harness.doc.ai_panel.running_turn_mut().unwrap().findings = vec![
        finding(
            "表述",
            "搭配不当",
            Some(Fix {
                span: 0..3,
                before: "加强巡查力度不断提高".into(),
                after: "不断加大巡查力度".into(),
            }),
        ),
        finding("要素与格式", "缺少主送机关", None),
    ];
    harness
        .doc
        .ai_panel
        .finish(TurnState::Reported { fixes: 1 });
    harness.frame_texts();
    let texts = harness.frame_texts();
    for needle in [
        "问题清单",
        "表述（1）",
        "要素与格式（1）",
        "→ 改为「不断加大巡查力度」",
        "1 条有改法的已放进审校抽屉",
        "打开审校抽屉",
    ] {
        assert!(has(&texts, needle), "缺少「{needle}」：{texts:?}");
    }
    assert!(!harness.doc.ai_panel.running());
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
    harness.doc.ai_panel.composer.text =
        "根据以下要点起草一份通知：各区县做好冬季森林防火；12月1日前完成隐患排查；落实24小时值班。"
            .into();
    harness.with_page(|page| page.send_ai_panel());
    assert!(harness.doc.busy);

    let started = std::time::Instant::now();
    let mut batches = 0usize;
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
                harness.doc.ai_panel.append(&content, &reasoning, done);
            }
            crate::app::DocJob::ExportProgress(message) => {
                harness.doc.ai_panel.set_phase(&message);
            }
            crate::app::DocJob::AiTool(_)
            | crate::app::DocJob::AiNote(_)
            | crate::app::DocJob::AiWorkspace(_) => {}
            crate::app::DocJob::SkillDone(result) => {
                let crate::ai_panel::SkillResult::Proposal { draft: result, .. } =
                    *result.expect("提案应当生成成功")
                else {
                    panic!("要点写全了，不该停下来问");
                };
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
                assert!(batches > 2, "应当是分批到达的");
                assert!(!result.markdown.trim().is_empty());
                // 提案只是提案：正文一个字都没动。
                assert!(harness.doc.generated_markdown.is_empty());
                break;
            }
            other => panic!("意外的回投：{}", std::any::type_name_of_val(&other)),
        }
    }
}

/// 连真实模型与知识库，从侧栏发起研究式起草，检查后台线程回投的事件。默认忽略；
/// 环境变量同 `agent::engine` 的 `live_research_draft`。
#[test]
#[ignore = "需要真实模型与知识库"]
fn live_research_panel_round_trip() {
    let env = |key: &str| std::env::var(key).unwrap_or_default();
    if env("GONGWEN_LIVE_LLM_URL").is_empty() || env("GONGWEN_LIVE_KB_DIR").is_empty() {
        eprintln!("未设置联机测试的环境变量，跳过");
        return;
    }
    let mut harness = Harness::new("");
    crate::storage::set_test_config_dir(Some(env("GONGWEN_LIVE_KB_DIR").into()));
    let config = &mut harness.config;
    config.lm_studio.base_url = env("GONGWEN_LIVE_LLM_URL");
    config.lm_studio.model = env("GONGWEN_LIVE_LLM_MODEL");
    config.lm_studio.api_key = env("GONGWEN_LIVE_LLM_KEY");
    config.lm_studio.timeout_seconds = 300;
    config.rag.enabled = true;
    config.rag.embedding.base_url = env("GONGWEN_LIVE_EMBED_URL");
    config.rag.embedding.model = env("GONGWEN_LIVE_EMBED_MODEL");
    config.rag.embedding.api_key = env("GONGWEN_LIVE_EMBED_KEY");
    config.rag.rerank.mode = crate::models::RerankMode::None;
    let composer = &mut harness.doc.ai_panel.composer;
    composer.use_rag = true;
    // 带上「通知」，免得动笔前因文种卡住；受文对象也写明。
    composer.text = "起草一份通知，部署市直各单位开展人工智能辅助决策系统应用情况调研，\
                     简要介绍美军梅文项目的做法和教训作为参考，要求按时报送调研报告。"
        .into();
    harness.with_page(|page| page.send_ai_panel());
    assert!(harness.doc.busy);

    let started = std::time::Instant::now();
    let mut tools = 0usize;
    let mut workspaces = 0usize;
    loop {
        let message = harness
            ._keep
            .recv_timeout(Duration::from_secs(600))
            .expect("十分钟内应当有结果");
        let WorkerResult::Doc { job, .. } = message else {
            continue;
        };
        match job {
            crate::app::DocJob::AiTool(line) => {
                tools += 1;
                eprintln!("[{:>5.1}s] {line}", started.elapsed().as_secs_f32());
                harness.doc.ai_panel.step(line);
            }
            crate::app::DocJob::AiWorkspace(text) => {
                workspaces += 1;
                harness.doc.ai_panel.replace_content(text);
            }
            crate::app::DocJob::AiStream {
                content,
                reasoning,
                done,
            } => {
                harness.doc.ai_panel.append(&content, &reasoning, done);
            }
            crate::app::DocJob::AiNote(note) => eprintln!("        · {note}"),
            crate::app::DocJob::ExportProgress(phase) => harness.doc.ai_panel.set_phase(&phase),
            crate::app::DocJob::SkillDone(result) => {
                match *result.expect("研究式起草应当成功") {
                    crate::ai_panel::SkillResult::Suspended(run) => {
                        let texts: Vec<_> = run
                            .suspension
                            .questions
                            .iter()
                            .map(|q| q.text.clone())
                            .collect();
                        eprintln!("动笔前的问题：{texts:?}，按推荐项作答后接着跑");
                        // 照 apply_doc_job 的做法收下题目，再像用户点「确认」一样作答。
                        harness.doc.busy = false;
                        let id = harness
                            .doc
                            .ai_panel
                            .running_turn_mut()
                            .map(|t| t.id)
                            .unwrap();
                        harness.doc.ai_panel.ask(run);
                        harness.with_page(|page| page.answer_suspended(id));
                        assert!(harness.doc.busy, "答完应当接着跑");
                        continue;
                    }
                    crate::ai_panel::SkillResult::Report { .. } => panic!("起草不该交问题清单"),
                    crate::ai_panel::SkillResult::Proposal { draft, report, .. } => {
                        eprintln!(
                            "工具调用 {tools} 次，工作稿更新 {workspaces} 次，证据 {} 段，题 {} 道，用时 {:?}",
                            report.evidence.items().len(),
                            report.questions.len(),
                            started.elapsed()
                        );
                        assert!(tools > 3, "过程里应当有检索与缺口检查");
                        assert!(workspaces >= 1);
                        assert!(!draft.markdown.contains("[K"), "引用标记不进提案");
                        assert!(!draft.markdown.trim().is_empty());
                        assert!(harness.doc.generated_markdown.is_empty(), "正文没动");
                    }
                }
                break;
            }
            _ => {}
        }
    }
}

/// 连真实模型：没命中触发词时交模型选技能，选中润色后经引擎改写成提案。默认忽略；
/// 环境变量同 `live_panel_round_trip`（另可设 `GONGWEN_LIVE_LLM_KEY`）。
#[test]
#[ignore = "需要真实模型"]
fn live_model_routing_and_polish() {
    let (Ok(url), Ok(model)) = (
        std::env::var("GONGWEN_LIVE_LLM_URL"),
        std::env::var("GONGWEN_LIVE_LLM_MODEL"),
    ) else {
        eprintln!("未设置 GONGWEN_LIVE_LLM_URL / GONGWEN_LIVE_LLM_MODEL，跳过");
        return;
    };
    let document = "# 关于做好冬季森林防火工作的通知\n\n各区县：\n\n冬天到了，山上的草都干了，大家一定要把防火的事情当回事，\
                    12月1日前把隐患都查一遍，有问题马上整改。\n";
    let mut harness = Harness::new(document);
    harness.config.lm_studio.base_url = url;
    harness.config.lm_studio.model = model;
    harness.config.lm_studio.api_key = std::env::var("GONGWEN_LIVE_LLM_KEY").unwrap_or_default();
    harness.config.lm_studio.timeout_seconds = 300;
    // 不含任何触发词：两个技能都可用，发送后交模型判断。
    harness.doc.ai_panel.composer.text = "这段话太口语了，弄得正式一点".into();
    harness.with_page(|page| page.send_ai_panel());
    assert_eq!(harness.doc.ai_panel.turns[0].title, "自动选择技能");
    let started = std::time::Instant::now();
    loop {
        let message = harness
            ._keep
            .recv_timeout(Duration::from_secs(300))
            .expect("五分钟内应当有结果");
        let WorkerResult::Doc { job, .. } = message else {
            continue;
        };
        match job {
            crate::app::DocJob::AiTool(line) => {
                eprintln!("[{:>5.1}s] {line}", started.elapsed().as_secs_f32())
            }
            crate::app::DocJob::SkillDone(result) => {
                let crate::ai_panel::SkillResult::Proposal { skill, draft, .. } =
                    *result.expect("应当成功")
                else {
                    panic!("润色不该停下来问");
                };
                eprintln!(
                    "技能：{skill}，用时 {:?}\n{}",
                    started.elapsed(),
                    draft.markdown
                );
                assert_eq!(skill, "润色", "改写现有正文应当选润色");
                assert!(draft.markdown.contains("12月1日"), "事实锁定：时限不能丢");
                assert_eq!(harness.doc.generated_markdown, document, "正文没动");
                break;
            }
            _ => {}
        }
    }
}

#[test]
fn escape_closes_the_popup_until_the_trigger_is_typed_again() {
    let mut harness = Harness::new("# 标题\n\n正文\n");
    harness.doc.ai_panel.open = true;
    harness.doc.ai_panel.composer.text = "/".into();
    harness.focus_input();
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "↑↓ 选择"), "{texts:?}");
    harness.press(egui::Key::Escape);
    let texts = harness.frame_texts();
    assert!(!has(&texts, "↑↓ 选择"), "Esc 关掉：{texts:?}");
    assert_eq!(harness.doc.ai_panel.composer.text, "/", "Esc 不动文字");
    let id = egui::Id::new(("ai_panel_input", harness.doc.key));
    assert!(
        harness.ctx.memory(|memory| memory.has_focus(id)),
        "Esc 被弹出层吃掉，输入框不失焦"
    );
}

#[test]
fn at_lists_articles_by_group_and_leaves_a_reference_mark() {
    use crate::agent::board::{RefSource, Reference};
    use crate::ai_panel::mention::CatalogItem;
    use crate::models::TemplateKind;
    let mut harness = Harness::new("");
    harness.doc.ai_panel.open = true;
    let item = |source, id, title: &str, kind| CatalogItem {
        reference: Reference {
            source,
            id,
            title: title.into(),
        },
        kind,
        date: "2025-11-02".into(),
    };
    harness.doc.ai_panel.composer.popup.catalog = Some(vec![
        item(
            RefSource::Manuscript,
            7,
            "2025年冬季森林防火通知",
            TemplateKind::PhoneNotice,
        ),
        item(
            RefSource::Manuscript,
            8,
            "安全生产检查方案",
            TemplateKind::PlainDocument,
        ),
        item(
            RefSource::Knowledge,
            3,
            "森林防火条例",
            TemplateKind::PlainDocument,
        ),
    ]);
    harness.doc.ai_panel.composer.text = "参照@fh".into();
    harness.focus_input();
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "引用文章"), "{texts:?}");
    assert!(
        has(&texts, "稿件库") && has(&texts, "知识库"),
        "分组：{texts:?}"
    );
    assert!(
        has(&texts, "2025年冬季森林防火通知"),
        "拼音首字母过滤：{texts:?}"
    );
    assert!(has(&texts, "森林防火条例"), "{texts:?}");
    assert!(!has(&texts, "安全生产检查方案"), "{texts:?}");
    assert!(
        has(&texts, "电话通知 · 2025-11-02"),
        "带文种与日期：{texts:?}"
    );

    // ↓ 到第二项（知识库那篇），回车插入。
    harness.press(egui::Key::ArrowDown);
    harness.press(egui::Key::Enter);
    let composer = &harness.doc.ai_panel.composer;
    assert_eq!(composer.text, "参照@《森林防火条例》");
    assert_eq!(composer.refs.len(), 1);
    assert_eq!(composer.refs[0].source, RefSource::Knowledge);
    assert_eq!(composer.refs[0].id, 3);
    let texts = harness.frame_texts();
    assert!(has(&texts, "《森林防火条例》"), "底栏出文章标签：{texts:?}");

    harness.doc.ai_panel.composer.text.push_str("写今年的通知");
    harness.with_page(|page| page.send_ai_panel());
    let turn = harness.doc.ai_panel.turns.last().expect("开了一轮");
    assert_eq!(
        turn.prompt, "参照《森林防火条例》写今年的通知",
        "发出去的原话去掉 @"
    );
    assert!(turn.context.iter().any(|chip| chip == "《森林防火条例》"));
    let request = turn.request.as_ref().unwrap();
    assert_eq!(request.refs.len(), 1);
    assert!(
        harness.doc.ai_panel.composer.refs.is_empty(),
        "发出去后清空"
    );
    harness.with_page(|page| page.stop_ai_task());
}

#[test]
fn deleting_the_mark_drops_the_reference_and_the_chip_unlinks() {
    use crate::agent::board::{RefSource, Reference};
    let mut harness = Harness::new("");
    harness.doc.ai_panel.open = true;
    let reference = Reference {
        source: RefSource::Manuscript,
        id: 7,
        title: "甲通知".into(),
    };
    let composer = &mut harness.doc.ai_panel.composer;
    composer.refs = vec![reference.clone()];
    composer.text = "参照@《甲通知》写".into();
    harness.frame_texts();
    assert_eq!(harness.doc.ai_panel.composer.refs, [reference]);
    harness.doc.ai_panel.composer.text = "参照写".into();
    harness.frame_texts();
    assert!(
        harness.doc.ai_panel.composer.refs.is_empty(),
        "记号删了引用就没了"
    );
}

/// 侧栏输入框的样张：空框、`/` 弹出、`@` 弹出、带标签。出到 `tmp/ai-panel-*.png` 目视检查。
#[test]
#[ignore = "出样张，手动跑"]
fn composer_samples() {
    use crate::agent::board::{RefSource, Reference};
    use crate::ai_panel::mention::CatalogItem;
    use crate::models::TemplateKind;
    let size = egui::vec2(480.0, 720.0);
    let shoot = |harness: &mut Harness, name: &str| {
        let mut canvas = crate::ui_snapshot::Canvas::default();
        theme::configure_style(&harness.ctx);
        harness.ctx.set_pixels_per_point(2.0);
        for _ in 0..15 {
            let output = harness.frame_output(Vec::new(), size);
            canvas.absorb(&output.textures_delta);
        }
        let output = harness.frame_output(Vec::new(), size);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .join(format!("ai-panel-{name}.png"));
        canvas.render(&harness.ctx, output, size, theme::canvas(), &path);
        println!("{}", path.display());
    };

    let mut harness = Harness::new("");
    harness.with_page(|page| page.toggle_ai_panel(None));
    shoot(&mut harness, "empty");

    let mut harness = Harness::new("# 关于做好冬季森林防火工作的通知\n\n一、总体要求\n");
    harness.with_page(|page| page.toggle_ai_panel(None));
    harness.doc.ai_panel.composer.text = "/".into();
    harness.focus_input();
    shoot(&mut harness, "slash");

    let mut harness = Harness::new("");
    harness.with_page(|page| page.toggle_ai_panel(None));
    let item = |source, id, title: &str, kind| CatalogItem {
        reference: Reference {
            source,
            id,
            title: title.into(),
        },
        kind,
        date: "2025-11-02".into(),
    };
    harness.doc.ai_panel.composer.popup.catalog = Some(vec![
        item(
            RefSource::Manuscript,
            7,
            "关于做好2025年冬季森林防火工作的通知",
            TemplateKind::PlainDocument,
        ),
        item(
            RefSource::Manuscript,
            8,
            "关于开展安全生产大检查的通知",
            TemplateKind::PlainDocument,
        ),
        item(
            RefSource::Knowledge,
            3,
            "森林防火条例",
            TemplateKind::PlainDocument,
        ),
        item(
            RefSource::Knowledge,
            4,
            "应急物资管理办法",
            TemplateKind::OfficialLetter,
        ),
    ]);
    harness.doc.ai_panel.composer.text = "仿照@".into();
    harness.focus_input();
    shoot(&mut harness, "mention");

    let mut harness = Harness::new("# 关于做好冬季森林防火工作的通知\n\n一、总体要求\n");
    harness.with_page(|page| page.toggle_ai_panel(None));
    let composer = &mut harness.doc.ai_panel.composer;
    composer.text = "参照@《2025年冬季森林防火通知》压缩到800字".into();
    composer.refs = vec![Reference {
        source: RefSource::Manuscript,
        id: 7,
        title: "2025年冬季森林防火通知".into(),
    }];
    composer.selection = Some((0..10, "一、总体要求".into()));
    shoot(&mut harness, "chips");
    let texts = harness.frame_texts();
    assert!(
        !has(&texts, "仿写"),
        "引用的书名不把技能带偏到仿写：{texts:?}"
    );
}
