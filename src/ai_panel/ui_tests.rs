//! AI 侧栏界面的测试：发起、停止、选区、红线 1 与结果卡。

use super::*;
use crate::agent::skill::RESEARCH_DRAFT;
use crate::ai_panel::{TurnRequest, paste::PastedFile};
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
    store: Option<crate::manuscript::ManuscriptStore>,
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
            store: None,
            _keep,
        }
    }

    fn with_page<R>(&mut self, f: impl FnOnce(&mut DraftPage<'_>) -> R) -> R {
        let mut page = DraftPage {
            doc: &mut self.doc,
            config: &mut self.config,
            store: self.store.as_mut(),
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

/// 真正走 egui 粘贴事件：长文成为材料，短文仍在光标处插入；其它输入框的粘贴不接管。
#[test]
fn large_paste_becomes_a_file_only_in_the_focused_composer() {
    let mut harness = Harness::new("原有正文");
    harness.doc.ai_panel.open = true;
    harness.doc.ai_panel.composer.text = "请根据材料整理通知".into();
    harness.frame_texts();
    harness.focus_input();
    let text = "中文材料与🙂\n".repeat(400);
    harness.frame_with(vec![egui::Event::Paste(text.clone())]);
    let composer = &harness.doc.ai_panel.composer;
    assert_eq!(composer.text, "请根据材料整理通知");
    assert_eq!(composer.materials.len(), 1);
    let path = composer.materials[0].file.path().to_owned();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert_eq!(composer.materials[0].material.text, text);
    harness.frame_with(vec![egui::Event::Paste("短材料".into())]);
    assert!(harness.doc.ai_panel.composer.text.contains("短材料"));
    harness.ctx.memory_mut(|memory| {
        memory.surrender_focus(egui::Id::new(("ai_panel_input", harness.doc.key)))
    });
    harness.frame_with(vec![egui::Event::Paste(text)]);
    assert_eq!(harness.doc.ai_panel.composer.materials.len(), 1);
    harness.doc.ai_panel.composer.materials.clear();
    assert!(!path.exists(), "移除引用后清理临时文件");
}

#[test]
fn pasted_material_survives_serialization_and_file_cleanup() {
    let mut harness = Harness::new("原有正文");
    let file = PastedFile::new(&"材料全文🙂\n".repeat(400)).unwrap();
    let path = file.file.path().to_owned();
    let mut request = panel_request(Some(crate::agent::skill::POLISH), "按材料修改");
    request.materials.push(file.material.clone());
    let saved = serde_json::to_value(&request).unwrap();
    drop(file);
    assert!(!path.exists());
    let restored: TurnRequest = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(restored, request);
    let time = crate::prompt::TimeContext::now();
    let (_, board, ..) = harness
        .with_page(|page| page.prepare_skill(&restored, &time, None))
        .unwrap();
    assert!(board.request.contains(&request.materials[0].text));
    assert!(board.request.ends_with("【要求】\n按材料修改"));
    assert_eq!(harness.doc.generated_markdown, "原有正文");
    let mut old = saved;
    old.as_object_mut().unwrap().remove("materials");
    assert!(
        serde_json::from_value::<TurnRequest>(old)
            .unwrap()
            .materials
            .is_empty()
    );
}

/// 即使通过连续输入或短段粘贴积累长文，文字区也不能撑高、撑宽整个侧栏。
#[test]
fn composer_bounds_long_text_and_many_materials() {
    let mut harness = Harness::new("原有正文");
    harness.doc.ai_panel.composer.text = "很长的要求🙂没有空格".repeat(1000);
    for _ in 0..12 {
        harness
            .doc
            .ai_panel
            .composer
            .materials
            .push(PastedFile::new("材料").unwrap());
    }
    let ctx = harness.ctx.clone();
    for size in [egui::vec2(340.0, 600.0), egui::vec2(640.0, 900.0)] {
        for _ in 0..3 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    harness.with_page(|page| page.ai_composer_ui(ui));
                    assert!(ui.min_rect().height() < 350.0, "{}", ui.min_rect());
                    assert!(ui.min_rect().width() <= size.x, "{}", ui.min_rect());
                },
            );
            let mut sends = 0;
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Circle(circle) = &clipped.shape
                    && circle.fill == theme::accent()
                    && (circle.radius - 15.0).abs() < 0.1
                {
                    sends += 1;
                    let rect = egui::Rect::from_center_size(
                        circle.center,
                        egui::Vec2::splat(circle.radius * 2.0),
                    );
                    assert!(
                        clipped.clip_rect.contains_rect(rect),
                        "发送按钮被裁切：{rect}"
                    );
                    assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect));
                }
            }
            assert_eq!(sends, 1, "发送按钮应始终可见");
        }
    }
    harness.doc.ai_panel.open = true;
    let size = egui::vec2(340.0, 480.0);
    // egui 的底部 Panel 按上一帧内容定高；切换布局后先让面板尺寸收敛。
    for _ in 0..3 {
        harness.frame_output(Vec::new(), size);
    }
    for _ in 0..3 {
        let output = harness.frame_output(Vec::new(), size);
        let sends: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|clipped| {
                if let egui::epaint::Shape::Circle(circle) = &clipped.shape
                    && circle.fill == theme::accent()
                    && (circle.radius - 15.0).abs() < 0.1
                {
                    Some((
                        clipped.clip_rect,
                        egui::Rect::from_center_size(circle.center, egui::Vec2::splat(30.0)),
                    ))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(sends.len(), 1, "真实侧栏里的发送按钮应可见");
        assert!(
            sends[0].0.contains_rect(sends[0].1),
            "发送按钮被侧栏裁切：{sends:?}"
        );
    }
}

#[test]
fn workspace_is_shown_on_the_left_and_readonly_until_adopted() {
    let mut harness = Harness::new("原有正文");
    harness.doc.ai_panel.open = true;
    harness
        .doc
        .ai_panel
        .push_turn("起草".into(), "写稿".into(), vec![], None);
    harness
        .doc
        .ai_panel
        .begin_write(String::new(), String::new());
    harness.doc.ai_panel.append("左侧独有的工作稿", "", false);
    let sidebar = harness.frame_texts();
    assert!(has(&sidebar, "查看工作稿"));
    assert!(!has(&sidebar, "左侧独有的工作稿"), "侧栏不重复展示全文");
    let ctx = harness.ctx.clone();
    let mut drawn = false;
    ctx.memory_mut(|memory| {
        memory.request_focus(egui::Id::new(("ai_workspace_text", harness.doc.key, 1_u64)))
    });
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            events: vec![egui::Event::Text("不应写入".into())],
            ..Default::default()
        },
        |ui| harness.with_page(|page| drawn = page.ai_workspace_ui(ui)),
    );
    assert!(drawn);
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.text().contains("左侧独有的工作稿"))));
    assert_eq!(harness.doc.generated_markdown, "原有正文");
    assert_eq!(harness.doc.ai_panel.turns[0].content, "左侧独有的工作稿");
    harness.with_page(|page| page.stop_ai_task());
    assert_eq!(harness.doc.ai_panel.turns[0].content, "左侧独有的工作稿");
    assert_eq!(harness.doc.generated_markdown, "原有正文");
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

fn panel_request(skill: Option<&str>, text: &str) -> crate::ai_panel::TurnRequest {
    crate::ai_panel::TurnRequest {
        skill: skill.map(str::to_string),
        text: text.into(),
        selection: None,
        preset: None,
        use_rag: false,
        refs: Vec::new(),
        materials: Vec::new(),
        notes: Vec::new(),
        premise: None,
        on_proposal: false,
        style: Default::default(),
    }
}

#[test]
fn picking_cards_start_fully_ticked_and_show_the_details() {
    let mut harness = Harness::new("原有正文。");
    harness.doc.ai_panel.open = true;
    let skill = crate::agent::skill::parse(
        "pick-test",
        "---\nname: 多选测试\ntools: [ws.write]\nflow:\n  - step: confirm\n    over: matters\n    label: 来函事项\n    pick: true\n---\n",
        "测试",
    )
    .unwrap();
    let model = crate::agent::testkit::ScriptedModel::new(|_, _| String::new());
    let kb = crate::agent::testkit::KeywordKb::disabled();
    let mut board = crate::agent::board::Board::default();
    board.vars.insert(
        "matters".into(),
        serde_json::json!(["请协助提供经费", "请确定会议场地"]),
    );
    let mut driver = crate::agent::testkit::Driver::new(&skill, &model, &kb, board);
    let suspension = driver.run().unwrap();
    let request = panel_request(None, "复函");
    let panel = &mut harness.doc.ai_panel;
    let id = panel.push_turn("复函".into(), request.text.clone(), vec![], Some(request));
    panel.ask(Box::new(crate::ai_panel::SkillRun {
        skill,
        suspension,
        use_rag: false,
    }));
    assert_eq!(panel.turn_mut(id).unwrap().replies[0].picked, [0, 1]);
    harness.frame_texts();
    let texts = harness.frame_texts();
    for label in [
        "取舍来函事项",
        "已勾 2 / 2",
        "请确定会议场地",
        "全不选",
        "按勾选的来函事项继续",
    ] {
        assert!(has(&texts, label), "{label}: {texts:?}");
    }
    harness.doc.ai_panel.turn_mut(id).unwrap().replies[0]
        .picked
        .clear();
    let texts = harness.frame_texts();
    assert!(
        has(&texts, "一条都不勾，就是这些来函事项都不用"),
        "{texts:?}"
    );
}

#[test]
fn compare_cards_show_each_version_in_full() {
    let mut harness = Harness::new("原有正文。");
    harness.doc.ai_panel.open = true;
    let skill = crate::agent::skill::parse(
        "compare-test",
        "---\nname: 比选测试\nflow:\n  - step: compare\n    label: 开头段\n    count: 2\n    save_as: opening\n---\n## 方案\n写开头\n",
        "测试",
    )
    .unwrap();
    let model = crate::agent::testkit::ScriptedModel::new(|_, _| {
        "为切实做好森林防火工作，现通知如下。\n===\n入冬以来火险等级偏高，现通知如下。".into()
    });
    let kb = crate::agent::testkit::KeywordKb::disabled();
    let mut driver = crate::agent::testkit::Driver::new(
        &skill,
        &model,
        &kb,
        crate::agent::board::Board::default(),
    );
    let suspension = driver.run().unwrap();
    let request = panel_request(None, "写开头");
    let panel = &mut harness.doc.ai_panel;
    let id = panel.push_turn("起草".into(), request.text.clone(), vec![], Some(request));
    panel.ask(Box::new(crate::ai_panel::SkillRun {
        skill,
        suspension,
        use_rag: false,
    }));
    assert_eq!(
        panel.turn_mut(id).unwrap().replies[0].choice,
        None,
        "不替人挑"
    );
    harness.frame_texts();
    let texts = harness.frame_texts();
    for label in [
        "挑一个开头段",
        "方案一",
        "为切实做好森林防火工作，现通知如下。",
        "入冬以来火险等级偏高，现通知如下。",
        "用这个，继续",
    ] {
        assert!(has(&texts, label), "{label}: {texts:?}");
    }
}

#[test]
fn outline_confirmation_has_editing_and_refinement_controls_and_saves_the_instruction() {
    let mut harness = Harness::new("原有正文。");
    harness.doc.ai_panel.open = true;
    let skill = crate::agent::skill::builtin(crate::agent::skill::POLICY_REPORT).unwrap();
    let model = crate::agent::testkit::ScriptedModel::new(|_, prompt| {
        if prompt.contains("列出章的大纲") {
            "背景：研究问题".into()
        } else {
            "无".into()
        }
    });
    let kb = crate::agent::testkit::KeywordKb::disabled();
    let board = crate::agent::board::Board {
        clarified: true,
        draft: crate::models::DraftInput {
            kind: crate::models::TemplateKind::ResearchReport,
            title_hint: "人工智能应用研究".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut driver = crate::agent::testkit::Driver::new(&skill, &model, &kb, board);
    let suspension = driver.run().unwrap();
    let request = panel_request(Some(crate::agent::skill::POLICY_REPORT), "研究人工智能应用");
    let panel = &mut harness.doc.ai_panel;
    let id = panel.push_turn(
        "政策研究报告".into(),
        request.text.clone(),
        vec![],
        Some(request),
    );
    panel.ask(Box::new(crate::ai_panel::SkillRun {
        skill,
        suspension,
        use_rag: false,
    }));
    let reply = &mut panel.turn_mut(id).unwrap().replies[0];
    reply.custom = "背景：用户手工修改".into();
    reply.revise_instruction = "增加风险分析".into();
    let encoded = serde_json::to_string(reply).unwrap();
    let restored: crate::ai_panel::ReplyDraft = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.custom, "背景：用户手工修改");
    assert_eq!(restored.revise_instruction, "增加风险分析");
    let old: crate::ai_panel::ReplyDraft =
        serde_json::from_str(r#"{"choice":0,"custom":"旧大纲","skip":false}"#).unwrap();
    assert!(old.revise_instruction.is_empty());
    // 改名前存的会话：大纲的修改要求照样读回。
    let renamed: crate::ai_panel::ReplyDraft = serde_json::from_str(
        r#"{"choice":0,"custom":"旧大纲","skip":false,"outline_instruction":"旧要求","outline_base":"旧大纲"}"#,
    )
    .unwrap();
    assert_eq!(renamed.revise_instruction, "旧要求");
    assert_eq!(renamed.revise_base.as_deref(), Some("旧大纲"));
    let texts = harness.frame_texts();
    for label in [
        "确认大纲",
        "修改要求",
        "优化大纲",
        "确认大纲，开始写作",
        "背景：用户手工修改",
    ] {
        assert!(
            texts.iter().any(|text| text.contains(label)),
            "{label}: {texts:?}"
        );
    }
    assert_eq!(harness.doc.generated_markdown, "原有正文。");
    assert!(!harness.doc.busy);
    let panel = &mut harness.doc.ai_panel;
    let turn = panel.turn_mut(id).unwrap();
    let mut run = turn.run.take().unwrap();
    turn.replies[0].revise_base = Some(turn.replies[0].custom.clone());
    turn.resume();
    let texts = harness.frame_texts();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("正在优化，可以继续手工修改"))
    );
    let panel = &mut harness.doc.ai_panel;
    panel.turn_mut(id).unwrap().replies[0].custom = "背景：等待期间的新修改".into();
    run.suspension.questions[0].prefill = "背景：AI优化结果\n风险：新增风险分析".into();
    panel.ask(run);
    let reply = &panel.turn_mut(id).unwrap().replies[0];
    assert_eq!(reply.custom, "背景：等待期间的新修改");
    assert!(reply.revise_candidate.contains("新增风险分析"));
    assert_eq!(reply.revise_instruction, "增加风险分析");
    let texts = harness.frame_texts();
    assert!(texts.iter().any(|text| text.contains("未覆盖你的手工修改")));
}

/// 侧栏里开一轮「研究式起草」，停在动笔前的题上（检查点里的文种是出题时的文种）。
fn asking_turn(
    harness: &mut Harness,
    request: crate::ai_panel::TurnRequest,
    questions: Vec<crate::agent::clarify::Question>,
    at: usize,
) -> u64 {
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
        suspension: crate::agent::engine::Suspension {
            checkpoint: crate::agent::checkpoint::Checkpoint {
                at: vec![at],
                reason: crate::agent::checkpoint::Reason::Ask,
                label: String::new(),
                board,
                partial: false,
            },
            questions,
            save_as: None,
            decision: Default::default(),
        },
        use_rag: false,
    }));
    id
}

#[test]
fn predraft_answers_become_notes_and_the_same_turn_continues() {
    use crate::agent::clarify::predraft_questions;
    use crate::models::TemplateKind;
    let mut harness = Harness::new("");
    harness.doc.draft.kind = TemplateKind::OfficialLetter;
    harness.doc.ai_panel.open = true;
    let request = panel_request(Some(RESEARCH_DRAFT), "起草一份商洽函");
    let model_questions = vec![("篇幅多长？".to_string(), vec!["短".into(), "长".into()])];
    let questions = predraft_questions(model_questions, 3);
    let id = asking_turn(&mut harness, request, questions, 1);
    harness.doc.ai_panel.turn_mut(id).unwrap().replies[0].choice = Some(1);

    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "动笔前先确认这几件事"), "{texts:?}");
    assert!(has(&texts, "确认，开始起草"), "{texts:?}");

    harness.with_page(|page| page.answer_suspended(id));
    let turn = harness.doc.ai_panel.turn_mut(id).unwrap();
    assert!(turn.state.running(), "答完接着跑，同一轮");
    assert!(turn.questions.is_empty());
    assert!(turn.run.is_none(), "挂起的流程已带回后台");
    assert_eq!(turn.request.as_ref().unwrap().notes, ["篇幅多长？长"]);
    assert_eq!(harness.doc.ai_panel.turns.len(), 1, "不另开一轮");
    assert!(harness.doc.busy);
}

#[test]
fn switching_the_kind_reroutes_the_skill_in_the_same_card() {
    // 实测踩过：当前是公函时「政策研究报告」不参与选技能，落到研究式起草；切成研究报告后
    // 还带着研究式起草按公函出的题接着跑。现在切了文种就按新文种重新选技能、重新出题。
    use crate::agent::clarify::kind_question;
    use crate::models::TemplateKind;
    let mut harness = Harness::new("");
    harness.doc.draft.kind = TemplateKind::OfficialLetter;
    harness.doc.ai_panel.open = true;
    let request = panel_request(None, "请从中总结美军推进人工智能项目的得失，写一个研究报告");
    let question = kind_question(&request.text, TemplateKind::OfficialLetter).unwrap();
    let id = asking_turn(&mut harness, request, vec![question], 0);

    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(
        has(&texts, "先定文种，其余问题按定下的文种再问"),
        "{texts:?}"
    );
    assert!(has(&texts, "切换为研究报告（推荐）"), "{texts:?}");
    assert!(has(&texts, "确认文种，继续"), "{texts:?}");

    // 沿用替用户选好的推荐项「切换为研究报告」。
    harness.with_page(|page| page.answer_suspended(id));
    assert_eq!(harness.doc.draft.kind, TemplateKind::ResearchReport);
    assert_eq!(harness.doc.ai_panel.turns.len(), 1, "同一张卡片里重来");
    let turn = harness.doc.ai_panel.turn_mut(id).unwrap();
    assert_eq!(turn.title, "政策研究报告", "按新文种重新选技能");
    assert!(turn.state.running());
    assert!(turn.questions.is_empty() && turn.run.is_none());
    assert!(
        turn.notes.iter().any(|n| n.contains("已切换为研究报告")),
        "{:?}",
        turn.notes
    );
    assert_eq!(
        turn.request.as_ref().unwrap().premise,
        Some(TemplateKind::ResearchReport),
        "重新生成时不再问文种"
    );
    assert!(harness.doc.busy);
}

#[test]
fn answers_given_under_a_kind_changed_meanwhile_are_voided() {
    use crate::agent::clarify::predraft_questions;
    use crate::models::TemplateKind;
    let mut harness = Harness::new("");
    harness.doc.draft.kind = TemplateKind::OfficialLetter;
    harness.doc.ai_panel.open = true;
    let request = panel_request(Some(RESEARCH_DRAFT), "起草一份商洽函");
    let questions = predraft_questions(
        vec![("篇幅多长？".to_string(), vec!["短".into(), "长".into()])],
        3,
    );
    let id = asking_turn(&mut harness, request, questions, 1);
    harness.doc.ai_panel.turn_mut(id).unwrap().replies[0].choice = Some(1);
    // 题还没答，用户在要素区把文种改了。
    harness.doc.draft.kind = TemplateKind::PlainDocument;

    harness.with_page(|page| page.answer_suspended(id));
    assert_eq!(harness.doc.draft.kind, TemplateKind::PlainDocument);
    let turn = harness.doc.ai_panel.turn_mut(id).unwrap();
    assert!(turn.state.running(), "同一张卡片里按新文种重来");
    assert!(
        turn.request.as_ref().unwrap().notes.is_empty(),
        "按旧文种答的不落地"
    );
    assert!(
        turn.notes.iter().any(|n| n.contains("这批题作废")),
        "{:?}",
        turn.notes
    );
    assert_eq!(harness.doc.ai_panel.turns.len(), 1);
}

#[test]
fn research_answers_are_written_in_by_a_background_job_and_fall_back_when_the_model_fails() {
    use crate::agent::clarify::gap_questions;
    use crate::agent::gaps::{GapStatus, Ledger};
    let mut harness = Harness::new("");
    let raw = "# 关于冬季防火的通知\n\n## 工作安排\n\n请于【待核实：排查完成时限】前完成排查。\n"
        .to_string();
    let mut ledger = Ledger::default();
    ledger.sync(&raw, "", &[]);
    let questions = gap_questions(&ledger, 4, &[]);
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
        excluded: Default::default(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
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
    assert!(has(&texts, "「排查完成时限」定到什么时候？"), "{texts:?}");
    assert!(
        has(&texts, "所在：工作安排"),
        "题目要带上所在小节：{texts:?}"
    );

    // 模型连不上：闸门之前就失败，退回直接替换。
    harness.config.lm_studio.base_url = "http://127.0.0.1:9".into();
    harness.config.lm_studio.timeout_seconds = 5;
    harness.with_page(|page| page.apply_research_answers(id));
    assert!(harness.doc.busy, "要填的回答交后台模型写进段落");
    assert!(harness.doc.ai_panel.running());
    let mut steps = Vec::new();
    loop {
        let message = harness
            ._keep
            .recv_timeout(Duration::from_secs(60))
            .expect("一分钟内应当有结果");
        let WorkerResult::Doc { job, .. } = message else {
            continue;
        };
        match job {
            crate::app::DocJob::AiTool(line) => steps.push(line),
            crate::app::DocJob::GapRevised(revision) => {
                harness.doc.busy = false;
                crate::ai_panel::finish_research_revision(
                    &mut harness.doc,
                    &harness.config,
                    "按回答修订",
                    revision.before,
                    revision.research,
                    Some(revision.questions),
                );
                break;
            }
            _ => {}
        }
    }
    assert!(
        steps.iter().any(|line| line.contains("直接填入")),
        "{steps:?}"
    );
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
}

/// 待决事项（`docs/decision-modules.md` 第八节）：正文里的「【待核实】」在提案接受之后也能
/// 逐条填；答完出提案，正文不动；列出之后正文改过就重新列。
#[test]
fn pending_items_in_the_document_can_be_answered_into_a_proposal_later() {
    let body = "# 关于冬季防火的通知\n\n## 工作安排\n\n请于【待核实：排查完成时限】前完成排查。\n\n联系人：【待核实：联系人】。\n";
    let mut harness = Harness::new(body);
    harness.doc.ai_panel.open = true;
    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "正文里还有 2 处待核实"), "{texts:?}");
    assert!(has(&texts, "逐条填写"), "{texts:?}");

    let id = harness
        .doc
        .ai_panel
        .open_pending(body, &[])
        .expect("正文里有占位");
    let texts = harness.frame_texts();
    assert!(has(&texts, "待决事项"), "{texts:?}");
    assert!(has(&texts, "正文里这几处还没定"), "{texts:?}");
    assert!(has(&texts, "「排查完成时限」定到什么时候？"), "{texts:?}");
    assert!(!has(&texts, "逐条填写"), "已经列着，不再提示");
    assert_eq!(harness.doc.ai_panel.turns[0].state, TurnState::Pending);

    // 一题没填：正文不变，卡片留着。
    harness.with_page(|page| page.apply_pending_answers(id));
    assert!(harness.status.contains("都还没填"), "{}", harness.status);
    assert_eq!(harness.doc.ai_panel.turns.len(), 1);

    // 列出之后正文改过：这批回答不落地，按现在的正文重新列。
    harness.doc.ai_panel.turns[0].replies[0].custom = "12月1日".into();
    harness.doc.generated_markdown = body.replace("联系人：【待核实：联系人】。\n", "");
    harness.with_page(|page| page.apply_pending_answers(id));
    assert!(harness.status.contains("改过"), "{}", harness.status);
    let panel = &harness.doc.ai_panel;
    assert_eq!(panel.turns.len(), 1);
    assert_eq!(panel.turns[0].state, TurnState::Pending);
    assert_eq!(panel.turns[0].questions.len(), 1);
    assert!(panel.turns[0].replies[0].custom.is_empty());
    let id = panel.turns[0].id;

    // 填了一处：交后台写进段落（模型连不上，退回直接替换），出提案；待决卡片撤掉。
    harness.doc.ai_panel.turns[0].replies[0].custom = "12月1日".into();
    harness.config.lm_studio.base_url = "http://127.0.0.1:9".into();
    harness.config.lm_studio.timeout_seconds = 5;
    harness.with_page(|page| page.apply_pending_answers(id));
    assert!(harness.doc.busy, "要填的回答交后台模型写进段落");
    loop {
        let message = harness
            ._keep
            .recv_timeout(Duration::from_secs(60))
            .expect("一分钟内应当有结果");
        let WorkerResult::Doc { job, .. } = message else {
            continue;
        };
        if let crate::app::DocJob::GapRevised(revision) = job {
            harness.doc.busy = false;
            crate::ai_panel::finish_research_revision(
                &mut harness.doc,
                &harness.config,
                "按回答修订",
                revision.before,
                revision.research,
                Some(revision.questions),
            );
            break;
        }
    }
    let panel = &harness.doc.ai_panel;
    assert_eq!(panel.turns.len(), 1, "待决卡片交出去就撤掉");
    assert!(matches!(panel.turns[0].state, TurnState::Proposed(_)));
    assert!(panel.turns[0].prompt.contains("12月1日"));
    let proposal = harness.doc.ai_proposal.as_ref().unwrap();
    assert!(
        proposal.result.markdown.contains("请于12月1日前完成排查"),
        "{}",
        proposal.result.markdown
    );
    assert_eq!(proposal.before, harness.doc.generated_markdown);
    assert!(
        harness
            .doc
            .generated_markdown
            .contains("【待核实：排查完成时限】"),
        "仍然只是提案，正文没动"
    );
    // 提案没处理时不提示待决事项（正文还会变）。
    let texts = harness.frame_texts();
    assert!(!has(&texts, "处待核实"), "{texts:?}");
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
        has(&texts, "说说要做什么…"),
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
        has(&texts, "说说要做什么…"),
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
        excluded: Default::default(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
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

    assert!(harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
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
        excluded: Default::default(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));

    assert!(!harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
    assert_eq!(harness.doc.generated_markdown, before, "没勾确认不能落地");
    assert!(harness.doc.ai_proposal.is_some(), "提案原样留着");

    harness
        .doc
        .ai_proposal
        .as_mut()
        .unwrap()
        .fact_changes_confirmed = true;
    assert!(harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
    assert_eq!(harness.doc.generated_markdown, after);
}

#[test]
fn body_edited_after_proposal_blocks_acceptance() {
    let mut harness = Harness::new("# 标题\n\n原文。\n");
    let before = harness.doc.generated_markdown.clone();
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "润色".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before,
        result: GeneratedDraft {
            markdown: "# 标题\n\n润色后的原文。\n".into(),
            title: "标题".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes: Vec::new(),
        excluded: Default::default(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));
    // 提案搁着的时候用户又改了正文。
    let edited = "# 标题\n\n原文。\n\n用户后来补的一段。\n".to_string();
    harness.doc.generated_markdown = edited.clone();

    assert!(!harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
    assert_eq!(
        harness.doc.generated_markdown, edited,
        "用户后来的改动不能被冲掉"
    );
    assert!(harness.doc.ai_proposal.is_some(), "提案原样留着");
}

/// 排除一块后接受：正文是合并结果，审校提示按合并后的正文重算，被排除块里
/// 才有的版式问题不能跟进来（内核加固第 2 期，红线 3）。
#[test]
fn accepting_with_exclusions_merges_and_recomputes_warnings() {
    // 27 字宽的段落：第二行只挂一个句号，版式估算会报「末行挂单字」。
    let long_paragraph = format!("{}。", "长".repeat(26));
    let before = "第一段原样。\n\n中间不变。\n\n第三段原样。";
    let proposal_text = format!("{long_paragraph}\n\n中间不变。\n\n第三段改。");
    let mut harness = Harness::new(before);
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "润色".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: before.into(),
        result: GeneratedDraft {
            markdown: proposal_text.clone(),
            title: "标题".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes: Vec::new(),
        excluded: [0].into_iter().collect(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));
    // 整篇提案确实带着那条版式提示，排除后才应该消失。
    let full = crate::draft_page::reviewed_draft(
        &harness.doc.draft,
        &harness.config,
        &proposal_text,
        false,
    );
    assert!(
        full.warnings
            .iter()
            .any(|note| note.message.contains("挂单字")),
        "长段落应触发版式估算提示：{:?}",
        full.warnings
    );

    assert!(harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
    assert!(
        harness.doc.generated_markdown.contains("第一段原样。"),
        "被排除的块保留原文：{}",
        harness.doc.generated_markdown
    );
    assert!(
        harness.doc.generated_markdown.contains("第三段改。"),
        "其余块按提案落地：{}",
        harness.doc.generated_markdown
    );
    assert!(
        !harness
            .doc
            .warnings
            .iter()
            .any(|note| note.message.contains("挂单字")),
        "warnings 按合并后的正文重算：{:?}",
        harness.doc.warnings
    );
}

/// 事实联动：一块改日期、一块改措辞；排除改日期那块后事实变化为空，
/// 不勾确认也能接受（内核加固第 2 期）。
#[test]
fn excluding_the_fact_hunk_lifts_the_confirmation_gate() {
    let before =
        "# 通知\n\n请各单位于2025年10月1日前报送材料。\n\n二、工作要求\n\n请切实贯彻落实。";
    let after = "# 通知\n\n请各单位于2025年11月1日前报送材料。\n\n二、工作要求\n\n请全面贯彻落实。";
    let mut harness = Harness::new(before);
    let fact_changes =
        crate::ai_guard::compare_key_facts(before, after, &harness.config.vocabulary);
    assert!(!fact_changes.is_empty(), "日期变化应被认作关键事实变化");
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "改日期".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: before.into(),
        result: GeneratedDraft {
            markdown: after.into(),
            title: "通知".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes,
        excluded: [0].into_iter().collect(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));

    // 没勾确认，但改日期那块被排除了，合并后没有事实变化，可以落地。
    assert!(harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
    let landed = &harness.doc.generated_markdown;
    assert!(landed.contains("2025年10月1日"), "日期保留原文：{landed}");
    assert!(!landed.contains("2025年11月1日"), "{landed}");
    assert!(
        landed.contains("请全面贯彻落实。"),
        "措辞改动落地：{landed}"
    );
}

/// 排除一块后勾了确认（核对的是变短的清单），再把那块恢复：清单变长了，旧勾选不算数。
/// 审阅窗帧首会作废勾选，但左侧 AI 工作稿里的「不要这处」不经过那段逻辑，所以接受时
/// 要按「勾选时核对的清单是否覆盖当前事实变化」判定，不能只看勾选（红线 2、3）。
#[test]
fn a_confirmation_given_before_restoring_a_hunk_does_not_cover_its_facts() {
    let before = "# 通知\n\n请各单位于2025年10月1日前报送材料。\n\n二、工作要求\n\n请切实贯彻落实。\n\n三、经费\n\n本次安排经费100万元。";
    let after = "# 通知\n\n请各单位于2025年11月1日前报送材料。\n\n二、工作要求\n\n请切实贯彻落实。\n\n三、经费\n\n本次安排经费200万元。";
    let mut harness = Harness::new(before);
    let vocabulary = harness.config.vocabulary.clone();
    let fact_changes = crate::ai_guard::compare_key_facts(before, after, &vocabulary);
    // 只排除改日期的第一块，剩下经费那块的事实变化。
    let merged_after = "# 通知\n\n请各单位于2025年10月1日前报送材料。\n\n二、工作要求\n\n请切实贯彻落实。\n\n三、经费\n\n本次安排经费200万元。";
    let confirmed = crate::ai_guard::compare_key_facts(before, merged_after, &vocabulary);
    assert!(
        !confirmed.is_empty() && confirmed.len() < fact_changes.len(),
        "两块各带事实变化：{fact_changes:?} / {confirmed:?}"
    );
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "改日期和经费".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: before.into(),
        result: GeneratedDraft {
            markdown: after.into(),
            title: "通知".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes,
        // 已经恢复了改日期那块：当前不排除任何块。
        excluded: Default::default(),
        // 勾选是在排除改日期那块时打的，核对的是变短的清单。
        fact_changes_confirmed: true,
        confirmed_facts: confirmed,
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));

    assert!(!harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
    assert_eq!(
        harness.doc.generated_markdown, before,
        "日期变化没核对过，不能落地"
    );
    assert!(harness.doc.ai_proposal.is_some(), "提案原样留着");
}

/// 全部排除等于没接受：拦下、提案原样留着、正文不变（内核加固第 2 期）。
#[test]
fn excluding_every_hunk_blocks_acceptance() {
    let before = "# 通知\n\n原文。\n";
    let mut harness = Harness::new(before);
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "润色".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: before.into(),
        result: GeneratedDraft {
            markdown: "# 通知\n\n改过的原文。\n".into(),
            title: "通知".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes: Vec::new(),
        excluded: [0].into_iter().collect(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));

    assert!(!harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
    assert_eq!(harness.doc.generated_markdown, before, "正文不变");
    assert!(harness.doc.ai_proposal.is_some(), "提案原样留着");
    assert!(
        harness.status.contains("没有可接受的内容"),
        "{}",
        harness.status
    );
}

/// 提案被继续修订（重新装提案）后，上次挑的排除块作废（内核加固第 2 期）。
#[test]
fn reinstalling_a_proposal_clears_exclusions() {
    let mut harness = Harness::new("# 标题\n\n原文。\n");
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "润色".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: "# 标题\n\n原文。\n".into(),
        result: GeneratedDraft {
            markdown: "# 标题\n\n改过的原文。\n".into(),
            title: "标题".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes: Vec::new(),
        excluded: [0].into_iter().collect(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));

    let revised = GeneratedDraft {
        markdown: "# 标题\n\n再改一版的原文。\n".into(),
        title: "标题".into(),
        warnings: Vec::new(),
        proof_warnings: Vec::new(),
        proof_measured: false,
        files: Vec::new(),
    };
    GongwenApp::install_ai_proposal(
        &mut harness.doc,
        "# 标题\n\n原文。\n".into(),
        revised,
        "润色".into(),
        &harness.config.vocabulary,
    );
    let proposal = harness.doc.ai_proposal.as_ref().unwrap();
    assert!(proposal.excluded.is_empty(), "新提案按新的块重新挑");
    assert!(proposal.result.markdown.contains("再改一版"));
}

/// 结果卡记录：接受后看得出排除了几处，追问历史也带这句话（内核加固第 2 期）。
#[test]
fn the_result_card_records_how_many_hunks_were_excluded() {
    let before = "第一段原样。\n\n中间不变。\n\n第三段原样。";
    let mut harness = Harness::new(before);
    harness.doc.ai_panel.open = true;
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "润色".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: before.into(),
        result: GeneratedDraft {
            markdown: "第一段改。\n\n中间不变。\n\n第三段改。".into(),
            title: "标题".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes: Vec::new(),
        excluded: [0].into_iter().collect(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));

    assert!(harness.with_page(|page| GongwenApp::accept_ai_proposal(
        page.doc,
        page.config,
        page.status
    )));
    let turn = &harness.doc.ai_panel.turns[0];
    assert_eq!(turn.state, TurnState::Accepted);
    assert_eq!(turn.excluded_hunks, 1);
    let digest = crate::ai_panel::history::digest(1, turn).unwrap();
    assert!(digest.contains("排除了 1 处改动"), "{digest}");

    harness.frame_texts();
    let texts = harness.frame_texts();
    assert!(has(&texts, "已写入正文（排除 1 处）"), "{texts:?}");
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

/// 长列表与同名稿件都不能复用控件 ID；展开后输入框仍可编辑。
#[test]
fn article_popup_has_no_widget_id_clashes_with_long_or_duplicate_titles() {
    use crate::agent::board::{RefSource, Reference};
    use crate::ai_panel::mention::CatalogItem;
    let mut harness = Harness::new("");
    theme::configure_style(&harness.ctx);
    harness.doc.ai_panel.open = true;
    harness.doc.ai_panel.composer.popup.catalog = Some(
        (0..24)
            .map(|id| CatalogItem {
                reference: Reference {
                    source: RefSource::Manuscript,
                    id,
                    title: "关于推进人工智能应用与数据治理的研究报告".into(),
                },
                kind: crate::models::TemplateKind::ResearchReport,
                date: "2026-10-08".into(),
            })
            .collect(),
    );
    harness.doc.ai_panel.composer.text = "请根据 @".into();
    harness.focus_input();
    for _ in 0..5 {
        let texts = harness.frame_texts();
        assert!(
            !texts.iter().any(|text| text.contains("use of widget ID")),
            "控件 ID 冲突：{texts:?}"
        );
    }
    // 同一弹层在窗口缩窄、变矮、再放大后，仍留在侧栏里，且不遮输入文字。
    for size in [
        egui::vec2(480.0, 720.0),
        egui::vec2(340.0, 480.0),
        egui::vec2(640.0, 720.0),
    ] {
        for _ in 0..8 {
            harness.frame_output(Vec::new(), size);
        }
        let input_id = egui::Id::new(("ai_panel_input", harness.doc.key));
        let popup = harness
            .ctx
            .memory(|memory| memory.area_rect(input_id.with("popup")))
            .unwrap();
        let input = harness.ctx.read_response(input_id).unwrap().rect;
        let panel = harness
            .ctx
            .data(|data| {
                data.get_temp::<egui::containers::panel::PanelState>(egui::Id::new("ai_panel_v1"))
            })
            .unwrap()
            .outer_rect;
        assert!(
            egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(popup),
            "弹层越界：{size:?} {popup:?}"
        );
        assert!(
            popup.left() >= panel.left() && popup.right() <= panel.right(),
            "弹层超出侧栏：{popup:?} {panel:?}"
        );
        assert!(
            popup.bottom() <= input.top() - 16.0,
            "弹层遮住输入文字：{popup:?} {input:?}"
        );
        assert!(
            (popup.left() - (input.left() - 13.0)).abs() < 1.0,
            "没有对齐输入框：{popup:?} {input:?}"
        );
    }
    harness.press(egui::Key::ArrowDown);
    harness.press(egui::Key::ArrowDown);
    harness.press(egui::Key::Enter);
    assert_eq!(harness.doc.ai_panel.composer.refs[0].id, 2);
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
    // 同一上下文连续出样张时，字体纹理只在第一次上传，必须跨样张保留。
    let mut canvas = crate::ui_snapshot::Canvas::default();
    let mut shoot = |harness: &mut Harness, name: &str| {
        let size = if name.ends_with("narrow") {
            egui::vec2(340.0, 480.0)
        } else {
            size
        };
        theme::configure_style(&harness.ctx);
        harness.ctx.set_pixels_per_point(2.0);
        if name == "pasted-material-narrow" {
            let pasted = harness.frame_output(
                vec![egui::Event::Paste("这是一段很长的中文材料。\n".repeat(500))],
                size,
            );
            canvas.absorb(&pasted.textures_delta);
        }
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

    // 长列表、同名文章与悬停底色的实际样张；同时检查窄窗口的标题与快捷键分行。
    harness
        .doc
        .ai_panel
        .composer
        .popup
        .catalog
        .as_mut()
        .unwrap()
        .extend((100..116).map(|id| {
            item(
                RefSource::Knowledge,
                id,
                "关于推进人工智能应用与数据治理的研究报告",
                TemplateKind::ResearchReport,
            )
        }));
    shoot(&mut harness, "mention-long");
    let popup_id = egui::Id::new(("ai_panel_input", harness.doc.key)).with("popup");
    let popup = harness
        .ctx
        .memory(|memory| memory.area_rect(popup_id))
        .unwrap();
    harness.frame_output(
        vec![egui::Event::PointerMoved(egui::pos2(
            popup.left() + 80.0,
            popup.top() + 120.0,
        ))],
        size,
    );
    shoot(&mut harness, "mention-hover");
    shoot(&mut harness, "mention-narrow");

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

    let mut harness = Harness::new("原有正文");
    harness.with_page(|page| page.toggle_ai_panel(None));
    harness.doc.ai_panel.composer.text =
        "根据材料整理通知，保留原文数字和日期，列出待核实事项。\n".repeat(40);
    shoot(&mut harness, "long-input");
    shoot(&mut harness, "long-input-narrow");
    let mut harness = Harness::new("原有正文");
    harness.with_page(|page| page.toggle_ai_panel(None));
    harness.doc.ai_panel.composer.text = "请根据粘贴材料整理通知".into();
    harness.focus_input();
    shoot(&mut harness, "pasted-material-narrow");
}

/// 追问（16.15 B.7）：有待确认的提案时「再短一点」改的是提案；本会话答过的题并进已确认；
/// 之前的轮次进 `history`。
#[test]
fn follow_ups_work_on_the_pending_proposal_and_carry_the_session() {
    let mut harness = Harness::new("原来的正文，写得很长很长。\n");
    harness.doc.ai_panel.open = true;
    let earlier = crate::ai_panel::TurnRequest {
        skill: None,
        text: "写个通知".into(),
        selection: None,
        preset: None,
        use_rag: false,
        refs: Vec::new(),
        materials: Vec::new(),
        notes: vec!["会议时间：下周一".into()],
        premise: None,
        on_proposal: false,
        style: Default::default(),
    };
    harness
        .doc
        .ai_panel
        .push_turn("起草".into(), "写个通知".into(), vec![], Some(earlier));
    harness.doc.ai_panel.turns[0].content = "提案稿：关于开会的通知。".into();
    let draft = GeneratedDraft {
        markdown: "提案稿：关于开会的通知。\n".into(),
        title: String::new(),
        warnings: Vec::new(),
        proof_warnings: Vec::new(),
        proof_measured: false,
        files: Vec::new(),
    };
    let summary = GongwenApp::install_ai_proposal(
        &mut harness.doc,
        "原来的正文，写得很长很长。\n".into(),
        draft,
        "起草".into(),
        &[],
    );
    harness.doc.ai_panel.finish(TurnState::Proposed(summary));

    let request = crate::ai_panel::TurnRequest {
        skill: Some(crate::agent::skill::CONDENSE.into()),
        text: "再短一点".into(),
        selection: None,
        preset: None,
        use_rag: false,
        refs: Vec::new(),
        materials: Vec::new(),
        notes: Vec::new(),
        premise: None,
        on_proposal: true,
        style: Default::default(),
    };
    let time = crate::prompt::TimeContext::now();
    let (_, board, ..) = harness
        .with_page(|page| page.prepare_skill(&request, &time, None))
        .unwrap();
    assert_eq!(board.document, "提案稿：关于开会的通知。\n", "改的是提案");
    assert_eq!(board.workspace, board.document);
    assert_eq!(board.notes, ["会议时间：下周一"], "答过的题不再问");
    assert!(
        board.history.contains("【第 1 轮】你说：写个通知"),
        "{}",
        board.history
    );
    assert!(board.history.contains("交了提案，还没处理"));
    let turn = harness.doc.ai_panel.turns.last().unwrap();
    assert!(
        turn.context.iter().any(|c| c == "改提案"),
        "{:?}",
        turn.context
    );

    // 点掉「改提案」：改回正文。
    let mut request = request;
    request.on_proposal = false;
    let (_, board, ..) = harness
        .with_page(|page| page.prepare_skill(&request, &time, None))
        .unwrap();
    assert_eq!(board.document, "原来的正文，写得很长很长。\n");
}

/// 会话持久化（16.15 B）：关掉重开，侧栏照样开着，待确认的提案正文没动就重新装上；
/// 「新会话」从空白开始，「历史会话」能切回去。
#[test]
fn the_session_comes_back_after_reopening_the_manuscript() {
    use crate::manuscript::{ManuscriptStore, NewManuscript};
    let body = "原来的正文。\n";
    let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
    let id = store
        .create(
            &NewManuscript {
                content_markdown: body.into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let mut harness = Harness::new(body);
    harness.store = Some(store);
    harness.doc.manuscript_id = Some(id);
    harness.with_page(|page| page.sync_ai_session());
    harness.doc.ai_panel.open = true;
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "压一压".into(), vec![], None);
    harness.doc.ai_panel.turns[0].content = "压过的正文。".into();
    let draft = GeneratedDraft {
        markdown: "压过的正文。\n".into(),
        title: String::new(),
        warnings: Vec::new(),
        proof_warnings: Vec::new(),
        proof_measured: false,
        files: Vec::new(),
    };
    let summary =
        GongwenApp::install_ai_proposal(&mut harness.doc, body.into(), draft, "润色".into(), &[]);
    harness.doc.ai_panel.finish(TurnState::Proposed(summary));
    harness.with_page(|page| page.save_ai_session(true));

    // 「关掉重开」：同一个库、同一篇稿件，新的标签。
    let store = harness.store.take();
    let mut reopened = Harness::new(body);
    reopened.store = store;
    reopened.doc.manuscript_id = Some(id);
    reopened.with_page(|page| page.sync_ai_session());
    assert!(reopened.doc.ai_panel.open, "侧栏照样开着");
    assert_eq!(reopened.doc.ai_panel.turns.len(), 1);
    assert!(matches!(
        reopened.doc.ai_panel.turns[0].state,
        TurnState::Proposed(_)
    ));
    let proposal = reopened.doc.ai_proposal.as_ref().expect("提案重新装上了");
    assert_eq!(proposal.result.markdown, "压过的正文。\n");
    let first = reopened.doc.ai_panel.session.id.clone();

    // 新会话：空白、提案卸下；历史里能切回去，提案又装上。
    reopened.with_page(|page| page.new_ai_session());
    assert!(reopened.doc.ai_panel.turns.is_empty());
    assert!(reopened.doc.ai_proposal.is_none());
    reopened
        .doc
        .ai_panel
        .push_turn("审校".into(), "审一下".into(), vec![], None);
    reopened
        .doc
        .ai_panel
        .finish(TurnState::Reported { fixes: 0 });
    reopened.with_page(|page| page.refresh_ai_sessions());
    assert_eq!(reopened.doc.ai_panel.session.list.len(), 2);
    reopened.with_page(|page| page.open_ai_session(&first));
    assert_eq!(reopened.doc.ai_panel.session.id, first);
    assert_eq!(reopened.doc.ai_panel.turns[0].prompt, "压一压");
    assert!(reopened.doc.ai_proposal.is_some());
    // 切过来的会话成了当前：再开一次停在它上面。
    reopened.with_page(|page| page.save_ai_session(true));
    let current = reopened
        .store
        .as_ref()
        .unwrap()
        .list_ai_sessions(id)
        .unwrap()
        .into_iter()
        .find(|s| s.is_current)
        .unwrap();
    assert_eq!(current.id, first);
}

/// 风格（16.15 C.3）：写稿的技能排进风格并留一行过程；审核类不用；分不出时交模型挑。
#[test]
fn writing_skills_carry_the_chosen_style() {
    use crate::agent::style::{StyleExample, StyleProfile};
    use crate::agent::testkit::ScriptedModel;
    use crate::ai_panel::skill_job::{StylePick, apply_style};
    let profile = |name: &str| StyleProfile {
        name: name.into(),
        description: "总体基调：庄重。".into(),
        examples: vec![StyleExample {
            role: "开头".into(),
            text: "为深入贯彻落实……".into(),
            source: "防火".into(),
        }],
        ..StyleProfile::default()
    };
    let model = ScriptedModel::new(|_, _| "2".into());
    let polish = crate::agent::skill::builtin(crate::agent::skill::POLISH).unwrap();
    let review = crate::agent::skill::builtin(crate::agent::skill::REVIEW).unwrap();
    let mut lines = Vec::new();
    let mut emit = |event: crate::agent::engine::Event| {
        if let crate::agent::engine::Event::Tool(tool) = event {
            lines.push(tool.summary);
        }
    };

    let mut board = crate::agent::board::Board::default();
    let chosen = profile("部署通知");
    let used = apply_style(
        StylePick::Fixed(Box::new(chosen.clone()), "自动选"),
        &polish,
        &mut board,
        &model,
        &mut emit,
    );
    assert_eq!(used.as_deref(), Some(chosen.id.as_str()));
    assert!(
        board.style.starts_with("【写法风格：部署通知】"),
        "{}",
        board.style
    );
    assert!(board.style.contains("不得照搬"));

    let mut board = crate::agent::board::Board::default();
    let used = apply_style(
        StylePick::Fixed(Box::new(profile("部署通知")), "指定"),
        &review,
        &mut board,
        &model,
        &mut emit,
    );
    assert!(used.is_none() && board.style.is_empty(), "审核类不用风格");

    let mut board = crate::agent::board::Board::default();
    let tie = vec![profile("甲"), profile("乙")];
    let used = apply_style(
        StylePick::Ask(tie.clone()),
        &polish,
        &mut board,
        &model,
        &mut emit,
    );
    assert_eq!(used.as_deref(), Some(tie[1].id.as_str()), "模型挑了第 2 份");
    assert!(
        lines.iter().any(|l| l == "风格：部署通知（自动选）"),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("风格：乙（模型从几份里挑的）")),
        "{lines:?}"
    );
}

/// 会话与风格的样张：标题行（会话名、历史会话、新会话）、压缩摘要行、「改提案」「风格」标签、
/// 已中断与已过期的卡片。出到 `tmp/ai-panel-session.png` 目视检查。
#[test]
#[ignore = "出样张，手动跑"]
fn session_samples() {
    use crate::agent::style::StyleProfile;
    let size = egui::vec2(480.0, 900.0);
    let mut harness = Harness::new("# 关于做好冬季森林防火工作的通知\n\n一、总体要求\n");
    harness.with_page(|page| page.toggle_ai_panel(None));
    let panel = &mut harness.doc.ai_panel;
    panel.styles = vec![StyleProfile {
        name: "对下部署类通知".into(),
        ..StyleProfile::default()
    }];
    panel.session.summary = "前面起草了一份冬季森林防火通知，用户确认会议时间为下周一。".into();
    for (title, prompt) in [("起草", "写个防火通知"), ("精简", "压一压")] {
        panel.push_turn(title.into(), prompt.into(), vec![], None);
        panel.finish(TurnState::Accepted);
    }
    panel.session.compacted_upto = 2;
    panel.push_turn("润色".into(), "语气再严一点".into(), vec![], None);
    panel.finish(TurnState::Interrupted);
    panel.push_turn(
        "精简".into(),
        "再短一点".into(),
        vec!["改提案".into()],
        None,
    );
    panel.turns.last_mut().unwrap().content =
        "# 关于做好冬季森林防火工作的通知\n\n各地要压实责任。".into();
    let draft = GeneratedDraft {
        markdown: "# 关于做好冬季森林防火工作的通知\n\n各地要压实责任。\n".into(),
        title: String::new(),
        warnings: Vec::new(),
        proof_warnings: Vec::new(),
        proof_measured: false,
        files: Vec::new(),
    };
    let before = harness.doc.generated_markdown.clone();
    let summary =
        GongwenApp::install_ai_proposal(&mut harness.doc, before, draft, "精简".into(), &[]);
    harness.doc.ai_panel.finish(TurnState::Proposed(summary));
    harness.doc.ai_panel.composer.text = "第二条展开说".into();
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
        .join("ai-panel-session.png");
    canvas.render(&harness.ctx, output, size, theme::canvas(), &path);
    println!("{}", path.display());
}

#[test]
fn a_generalized_gap_can_be_reverted_into_a_question() {
    use crate::agent::gaps::{GapStatus, Ledger};
    let mut harness = Harness::new("");
    let original = "依据【待核实：上级文件依据】，现就有关事项通知如下。";
    let generalized = "依据有关规定，现就有关事项通知如下。";
    let raw = format!("# 关于冬季防火的通知\n\n{generalized}\n");
    let mut ledger = Ledger::default();
    ledger.sync(&format!("# 关于冬季防火的通知\n\n{original}\n"), "", &[]);
    ledger.gaps[0].status = GapStatus::Generalized(original.into());
    ledger.gaps[0].sentence = generalized.into();
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
        excluded: Default::default(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    let panel = &mut harness.doc.ai_panel;
    panel.finish(TurnState::Proposed(ProposalSummary::default()));
    panel.turn_mut(id).unwrap().research = Some(crate::ai_panel::ResearchSnapshot {
        raw,
        ledger,
        sources: Vec::new(),
    });

    harness.with_page(|page| page.revert_generalized(id, 1));
    let panel = &harness.doc.ai_panel;
    let new_turn = panel.turns.last().unwrap();
    let research = new_turn.research.as_ref().unwrap();
    assert!(research.raw.contains(original), "{}", research.raw);
    assert_eq!(research.ledger.gaps[0].status, GapStatus::NoAnswer);
    assert_eq!(new_turn.questions.len(), 1, "撤回后改为出题问你");
    assert!(
        harness
            .doc
            .ai_proposal
            .as_ref()
            .unwrap()
            .result
            .markdown
            .contains("【待核实：上级文件依据】")
    );
}

/// 交付后确认题的样张：所在句、程序选项、AI 建议、自己写。出到 `tmp/ai-panel-questions.png`。
#[test]
#[ignore = "出样张，手动跑"]
fn question_samples() {
    use crate::agent::clarify::{add_suggestions, gap_questions};
    use crate::agent::gaps::Ledger;
    let size = egui::vec2(480.0, 1500.0);
    let mut harness = Harness::new("");
    let raw = "# 关于商请共建公共数据研究平台的函\n\n市数据局：\n\n## 工作安排\n\n\
               请贵单位于【待核实：研究方案反馈时限】前反馈研究意向、任务分工与资源需求。\
               跨部门共享的数据须以【待核实：报送方式】报送。\n\n联系人：【待核实：联系人】。\n"
        .to_string();
    let mut ledger = Ledger::default();
    ledger.sync(&raw, "", &[]);
    let mut questions = gap_questions(&ledger, 4, &[]);
    add_suggestions(
        &mut questions,
        "1｜收到本函后15个工作日内｜2026年10月31日前\n2｜书面函复｜电子邮件",
    );
    harness.doc.ai_panel.open = true;
    let panel = &mut harness.doc.ai_panel;
    let id = panel.push_turn("研究式起草".into(), "写个函".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: String::new(),
        result: GeneratedDraft {
            markdown: raw.clone(),
            title: "关于商请共建公共数据研究平台的函".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "研究式起草".into(),
        fact_changes: Vec::new(),
        excluded: Default::default(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    let panel = &mut harness.doc.ai_panel;
    panel.finish(TurnState::Proposed(ProposalSummary::default()));
    let turn = panel.turn_mut(id).unwrap();
    turn.replies = crate::ai_panel::initial_replies(&questions);
    // 第一题点了 AI 建议，第三题勾了保留待核实。
    turn.replies[0].custom = "收到本函后15个工作日内".into();
    turn.replies[2].skip = true;
    turn.questions = questions;
    turn.research = Some(crate::ai_panel::ResearchSnapshot {
        raw,
        ledger,
        sources: Vec::new(),
    });

    let mut canvas = crate::ui_snapshot::Canvas::default();
    theme::configure_style(&harness.ctx);
    harness.ctx.set_pixels_per_point(2.0);
    for _ in 0..15 {
        let output = harness.frame_output(Vec::new(), size);
        canvas.absorb(&output.textures_delta);
    }
    // 滚到确认题那里，题卡全在视野里。
    for _ in 0..1 {
        let output = harness.frame_output(
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -400.0),
                modifiers: egui::Modifiers::default(),
                phase: egui::TouchPhase::Move,
            }],
            size,
        );
        canvas.absorb(&output.textures_delta);
    }
    let output = harness.frame_output(Vec::new(), size);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tmp")
        .join("ai-panel-questions.png");
    canvas.render(&harness.ctx, output, size, theme::canvas(), &path);
    println!("{}", path.display());
}

// —— 内核加固第 4 期：检查点与统一恢复入口 ——

/// 一个挂起在检查点上的研究式起草（崩溃恢复用同一份现场）。
fn suspended_draft(harness: &mut Harness, date: &str) -> (u64, crate::ai_panel::TurnRequest) {
    use crate::agent::checkpoint::{Checkpoint, Reason};
    use crate::models::TemplateKind;
    let request = crate::ai_panel::TurnRequest {
        skill: Some(RESEARCH_DRAFT.into()),
        text: "起草一份商洽函".into(),
        selection: None,
        preset: None,
        use_rag: false,
        refs: Vec::new(),
        materials: Vec::new(),
        notes: Vec::new(),
        premise: None,
        on_proposal: false,
        style: Default::default(),
    };
    let mut draft = harness.doc.draft.clone();
    draft.kind = TemplateKind::PlainDocument;
    draft.date = date.into();
    let board = crate::agent::board::Board {
        draft,
        request: request.text.clone(),
        // 检查点里存的是「昨天」的系统提示与日期；恢复时都该被界面当前值盖掉。
        system_prompt: "昨天写的系统提示".into(),
        time_sources: "昨天".into(),
        history: "上一轮：交给了提案，还没处理".into(),
        ..Default::default()
    };
    let panel = &mut harness.doc.ai_panel;
    panel.push_turn(
        "研究式起草".into(),
        request.text.clone(),
        vec![],
        Some(request.clone()),
    );
    panel.ask(Box::new(crate::ai_panel::SkillRun {
        skill: crate::agent::skill::builtin(RESEARCH_DRAFT).unwrap(),
        suspension: crate::agent::engine::Suspension {
            checkpoint: Checkpoint {
                at: vec![1],
                reason: Reason::Ask,
                label: "已完成 算子 clarify".into(),
                board,
                partial: false,
            },
            questions: Vec::new(),
            save_as: None,
            decision: Default::default(),
        },
        use_rag: false,
    }));
    // 崩溃：这一轮不再等回答，界面回到「已中断」。
    panel.turns.clear();
    let id = panel.push_turn(
        "研究式起草".into(),
        request.text.clone(),
        vec![],
        Some(request.clone()),
    );
    panel.finish(TurnState::Interrupted);
    (id, request)
}

/// 恢复前在界面上改了成文日期，进模型的是新值；系统提示与日期按现在重建，
/// `history` 照旧沿用（这一轮任务本身的输入，不重灌）。
#[test]
fn resuming_refreshes_the_authorised_elements_and_dates_from_the_ui() {
    use crate::agent::board::Board;
    let time = crate::prompt::TimeContext::now();
    let mut board = Board {
        draft: crate::models::DraftInput {
            date: "2025年1月1日".into(),
            ..Default::default()
        },
        document: "旧正文".into(),
        system_prompt: "昨天写的系统提示".into(),
        time_sources: "昨天".into(),
        request: "起草".into(),
        selection: Some("锁的选区".into()),
        preset: "精简篇幅".into(),
        history: "上一轮：交了提案".into(),
        style: "写法风格".into(),
        ..Board::default()
    };
    let ui_draft = crate::models::DraftInput {
        date: "2026年10月7日".into(),
        ..Default::default()
    };
    board.refresh_from_ui(&ui_draft, "现在的正文", &time);

    assert_eq!(board.draft.date, "2026年10月7日", "成文日期用界面上新改的");
    assert_eq!(board.document, "现在的正文");
    assert_ne!(board.system_prompt, "昨天写的系统提示");
    assert!(board.time_sources.contains(&time.today));
    // 这一轮任务本身的输入不重灌。
    assert_eq!(board.request, "起草");
    assert_eq!(board.selection.as_deref(), Some("锁的选区"));
    assert_eq!(board.preset, "精简篇幅");
    assert_eq!(board.history, "上一轮：交了提案");
    assert_eq!(board.style, "写法风格");
}

/// 崩溃 / 停止 / 出错后的「接着跑」：从库里取最新检查点，走与答完题同一个 `start_skill` 分支。
#[test]
fn a_crashed_turn_resumes_from_the_stored_checkpoint() {
    use crate::manuscript::{ManuscriptStore, NewManuscript};
    let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
    let manuscript = store
        .create(
            &NewManuscript {
                content_markdown: "正文。\n".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let mut harness = Harness::new("正文。\n");
    harness.store = Some(store);
    harness.doc.manuscript_id = Some(manuscript);
    harness.with_page(|page| page.sync_ai_session());
    let session = harness.doc.ai_panel.session.id.clone();

    let (id, request) = suspended_draft(&mut harness, "2025年1月1日");
    // 检查点的外键挂在轮次上，先把这一轮存进会话（开跑前 `start_skill` 强制存的那一次）。
    harness.with_page(|page| page.save_ai_session(true));
    // 模拟后台线程落下的检查点（第一遍跑过了 clarify，崩在下一步之前）。
    {
        let store = harness.store.as_mut().unwrap();
        let checkpoint = crate::agent::checkpoint::Checkpoint {
            at: vec![1],
            reason: crate::agent::checkpoint::Reason::Step,
            label: "已完成 算子 clarify".into(),
            board: crate::agent::board::Board {
                request: "起草一份商洽函".into(),
                workspace: "写到一半的工作稿".into(),
                ..Default::default()
            },
            partial: false,
        };
        store
            .save_run_checkpoint(
                &session,
                id as i64,
                RESEARCH_DRAFT,
                "hash",
                false,
                &checkpoint,
            )
            .unwrap();
    }
    // 用户在恢复前改了成文日期。
    harness.doc.draft.date = "2026年10月7日".into();

    harness.with_page(|page| page.resume_checkpoint(id));
    let turn = harness.doc.ai_panel.turn_mut(id).expect("还是同一轮");
    assert!(turn.state.running(), "接着跑了");
    assert!(turn.run.is_none(), "检查点已带回后台");
    assert!(
        turn.notes.iter().any(|n| n.contains("已完成 算子 clarify")),
        "任务流里插一行从哪儿接着跑：{:?}",
        turn.notes
    );
    assert_eq!(
        harness.doc.ai_panel.turns[0].content, "写到一半的工作稿",
        "界面显示的工作稿回到检查点里的样子"
    );
    assert_eq!(
        harness.doc.ai_panel.turns[0].request.as_ref().unwrap(),
        &request,
        "沿用发起时的请求"
    );
    assert!(harness.doc.busy);
}

/// 找不到检查点时不假装能续，状态栏说清楚。
#[test]
fn resuming_without_a_checkpoint_says_so_instead_of_pretending() {
    use crate::manuscript::{ManuscriptStore, NewManuscript};
    let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
    let manuscript = store
        .create(
            &NewManuscript {
                content_markdown: "正文。\n".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let mut harness = Harness::new("正文。\n");
    harness.store = Some(store);
    harness.doc.manuscript_id = Some(manuscript);
    harness.with_page(|page| page.sync_ai_session());
    let (id, _) = suspended_draft(&mut harness, "2025年1月1日");
    harness.with_page(|page| page.save_ai_session(true));

    harness.with_page(|page| page.resume_checkpoint(id));
    assert!(
        harness.status.contains("没有找到检查点"),
        "{}",
        harness.status
    );
    let turn = harness.doc.ai_panel.turn_mut(id).expect("还是同一轮");
    assert!(
        !turn.state.running(),
        "没有凭空跑起来，状态仍是 {:?}",
        turn.state
    );
}

/// 红线回归（验收 5）：从检查点续跑出来的工作稿，照样被关键事实比对挡下——
/// 检查点存的现场不能绕过闸门直接落地（续跑与正常跑完是同一条 `Outcome::Done` 路径）。
#[test]
fn a_resumed_workspace_still_goes_through_the_key_fact_gate() {
    use crate::agent::testkit::{Driver, KeywordKb, ScriptedModel};

    let skill = crate::agent::skill::builtin(crate::agent::skill::POLISH).unwrap();
    let model = ScriptedModel::new(|_, _| String::new());
    let kb = KeywordKb::disabled();
    // 检查点里存着的工作稿：正文里冒出材料没有的日期与人数。
    let stored = crate::agent::board::Board {
        request: "压一压".into(),
        document: "原正文。\n".into(),
        workspace: "# 关于开展安全生产检查的通知\n\n各单位定于2027年3月18日派出工作组共5人。\n"
            .into(),
        ..Default::default()
    };
    let mut driver = Driver::new(&skill, &model, &kb, stored.clone());
    // 从检查点给的路径恢复（这里指向流程末尾，模拟「最后一步之后崩了」）。
    assert!(driver.run_from(&[usize::MAX]).is_none(), "恢复后跑完了");

    // 定稿闸门（`reviewed_draft` 之后的关键事实比对，Outcome::Done 分支里跑的那个）：
    // 来源不明的日期与人数照样被记成变化。
    let changes =
        crate::ai_guard::compare_key_facts(&stored.document, &driver.board.workspace, &[]);
    assert!(
        changes.iter().any(|change| change.value.contains("2027")
            && change.kind != crate::ai_guard::FactKind::Document),
        "来源不明的日期照样被记成事实变化：{changes:?}"
    );
}

/// 验收 6 的另一半：恢复时系统提示按现在重建（不含旧的历史块），后台再拼一次历史，
/// 会话历史在 `system_prompt` 里只出现一次。
#[test]
fn the_session_history_lands_in_the_system_prompt_exactly_once_after_resuming() {
    use crate::agent::board::Board;
    let time = crate::prompt::TimeContext::now();
    // 检查点里存的是第一遍跑完后拼过历史、又崩掉的 system_prompt。
    let mut board = Board {
        system_prompt: format!(
            "{}\\n\\n【本会话之前的往来】\\n第一轮：写了通知",
            crate::prompt::build_system_prompt(&time)
        ),
        history: "第一轮：写了通知".into(),
        ..Board::default()
    };
    // 恢复：系统提示按现在重建（`refresh_from_ui` 覆盖掉旧的历史块）。
    board.refresh_from_ui(&Default::default(), "", &time);
    assert_eq!(
        board.system_prompt.matches("【本会话之前的往来】").count(),
        0,
        "重建后的系统提示是干净的"
    );
    // 后台线程只再拼一次。
    let with = crate::ai_panel::skill_job::with_history(&board.system_prompt, &board.history);
    assert_eq!(
        with.matches("【本会话之前的往来】").count(),
        1,
        "历史只出现一次：{with}"
    );
}

/// 验收 9：`Interrupted` / `Stopped` / `Failed` 的卡片有检查点时出「接着跑」，没有时保持原样。
#[test]
fn interrupted_cards_offer_resume_only_when_a_checkpoint_exists() {
    use crate::manuscript::{ManuscriptStore, NewManuscript};
    let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
    let manuscript = store
        .create(
            &NewManuscript {
                content_markdown: "正文。\n".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let mut harness = Harness::new("正文。\n");
    harness.store = Some(store);
    harness.doc.manuscript_id = Some(manuscript);
    harness.doc.ai_panel.open = true;
    harness.with_page(|page| page.sync_ai_session());
    let session = harness.doc.ai_panel.session.id.clone();

    // 没有检查点：只有「重新生成」，不假装能续。
    harness
        .doc
        .ai_panel
        .push_turn("研究式起草".into(), "起草".into(), vec![], Some(request()));
    let id = harness.doc.ai_panel.turns[0].id;
    harness.doc.ai_panel.finish(TurnState::Interrupted);
    harness.with_page(|page| page.refresh_resumable());
    let texts = harness.frame_texts();
    assert!(has(&texts, "程序关闭时这一轮还没跑完"), "{texts:?}");
    assert!(has(&texts, "重新生成"), "{texts:?}");
    assert!(!has(&texts, "接着跑"), "没有检查点就不给接着跑：{texts:?}");

    // 落一份检查点：卡片出「接着跑 / 从头重来 / 丢弃」，并注明停在哪一步。
    harness.with_page(|page| page.save_ai_session(true));
    {
        let store = harness.store.as_mut().unwrap();
        store
            .save_run_checkpoint(
                &session,
                id as i64,
                RESEARCH_DRAFT,
                "hash",
                false,
                &crate::agent::checkpoint::Checkpoint {
                    at: vec![1],
                    reason: crate::agent::checkpoint::Reason::Step,
                    label: "已完成 算子 clarify".into(),
                    board: crate::agent::board::Board::default(),
                    partial: false,
                },
            )
            .unwrap();
    }
    harness.with_page(|page| page.refresh_resumable());
    let texts = harness.frame_texts();
    assert!(has(&texts, "接着跑"), "{texts:?}");
    assert!(has(&texts, "从头重来"), "{texts:?}");
    assert!(has(&texts, "丢弃"), "{texts:?}");
    assert!(
        has(&texts, "可接着跑：已完成 算子 clarify"),
        "注明停在哪一步：{texts:?}"
    );

    // 丢弃：检查点删掉，卡片回到只有「重新生成」。
    harness.with_page(|page| page.drop_checkpoints(id));
    assert!(
        harness
            .store
            .as_ref()
            .unwrap()
            .list_run_checkpoints(&session, id as i64)
            .unwrap()
            .is_empty(),
        "库里删干净了"
    );
    let texts = harness.frame_texts();
    assert!(!has(&texts, "接着跑"), "{texts:?}");
    assert!(has(&texts, "重新生成"), "{texts:?}");
}

/// Stopped 与 Failed 也一样：找到检查点就能接着跑（提示词第 3 节的出入 3）。
#[test]
fn stopped_and_failed_cards_also_offer_resume() {
    for state in [TurnState::Stopped, TurnState::Failed("模型超时".into())] {
        let mut harness = Harness::new("");
        harness.doc.ai_panel.open = true;
        harness
            .doc
            .ai_panel
            .push_turn("研究式起草".into(), "起草".into(), vec![], Some(request()));
        let id = harness.doc.ai_panel.turns[0].id;
        harness.doc.ai_panel.finish(state.clone());
        // 直接标成可续（库里那份检查点在真机上由后台线程落的）。
        harness.doc.ai_panel.turns[0].resumable = Some("已完成 算子 clarify · 10-07 15:04".into());
        let texts = harness.frame_texts();
        assert!(has(&texts, "接着跑"), "{state:?}：{texts:?}");
        assert!(
            has(&texts, "可接着跑：已完成 算子 clarify · 10-07 15:04"),
            "{state:?}：{texts:?}"
        );
        assert_eq!(id, 1);
    }
}

/// 一份够用的 `TurnRequest`（卡片上的「重新生成」要它）。
fn request() -> crate::ai_panel::TurnRequest {
    crate::ai_panel::TurnRequest {
        skill: Some(RESEARCH_DRAFT.into()),
        text: "起草".into(),
        selection: None,
        preset: None,
        use_rag: false,
        refs: Vec::new(),
        materials: Vec::new(),
        notes: Vec::new(),
        premise: None,
        on_proposal: false,
        style: Default::default(),
    }
}

/// 「从这里重跑」：正文没动过才允许，且会覆盖该点之后的检查点。
#[test]
fn rerunning_from_an_older_checkpoint_requires_an_untouched_document() {
    use crate::agent::checkpoint::{Checkpoint, Reason};
    use crate::manuscript::{ManuscriptStore, NewManuscript};
    let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
    let manuscript = store
        .create(
            &NewManuscript {
                content_markdown: "正文。\n".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let mut harness = Harness::new("正文。\n");
    harness.store = Some(store);
    harness.doc.manuscript_id = Some(manuscript);
    harness.doc.ai_panel.open = true;
    harness.with_page(|page| page.sync_ai_session());
    let session = harness.doc.ai_panel.session.id.clone();
    harness
        .doc
        .ai_panel
        .push_turn("研究式起草".into(), "起草".into(), vec![], Some(request()));
    let id = harness.doc.ai_panel.turns[0].id;
    harness.doc.ai_panel.finish(TurnState::Interrupted);
    harness.with_page(|page| page.save_ai_session(true));
    let ckpt = |at: usize| Checkpoint {
        at: vec![at],
        reason: Reason::Step,
        label: format!("已完成 第 {at} 步"),
        board: crate::agent::board::Board {
            document: "正文。\n".into(),
            ..Default::default()
        },
        partial: false,
    };
    for at in 1..=3 {
        harness
            .store
            .as_mut()
            .unwrap()
            .save_run_checkpoint(
                &session,
                id as i64,
                RESEARCH_DRAFT,
                "hash",
                false,
                &ckpt(at),
            )
            .unwrap();
    }

    // 正文动过：拒绝，并说清原因。
    harness.doc.generated_markdown = "用户自己改过的正文。\n".into();
    harness.with_page(|page| page.rerun_from_checkpoint(id, 1));
    assert!(
        harness.status.contains("正文在那一步之后改过了"),
        "{}",
        harness.status
    );
    assert!(!harness.doc.busy, "没有跑起来");
    assert_eq!(
        harness
            .store
            .as_ref()
            .unwrap()
            .list_run_checkpoints(&session, id as i64)
            .unwrap()
            .len(),
        3,
        "被拒时不删任何东西"
    );

    // 正文没动过：允许，从第 1 步之后重跑，并覆盖该点之后的检查点。
    harness.doc.generated_markdown = "正文。\n".into();
    harness.with_page(|page| page.rerun_from_checkpoint(id, 1));
    assert!(harness.doc.busy, "跑起来了");
    let left = harness
        .store
        .as_ref()
        .unwrap()
        .list_run_checkpoints(&session, id as i64)
        .unwrap();
    assert_eq!(left.len(), 1, "该点之后的检查点删掉了");
    assert_eq!(left[0].checkpoint.at, [1]);
    let turn = harness.doc.ai_panel.turn_mut(id).unwrap();
    assert!(turn.state.running());
    assert!(
        turn.notes.iter().any(|n| n.contains("已完成 第 1 步")),
        "任务流里说明从哪儿接着跑：{:?}",
        turn.notes
    );
}

/// 「把 AI 工作稿提交到正文」不发给模型（模型写不了正文），改出「写入正文？」确认卡：
/// 不开新一轮（新一轮会把挂着的提案作废），提案与正文都原样不动，等用户点。
#[test]
fn saying_commit_to_body_shows_the_confirm_card_instead_of_asking_the_model() {
    let mut harness = Harness::new("# 标题\n\n原文。\n");
    let before = harness.doc.generated_markdown.clone();
    harness
        .doc
        .ai_panel
        .push_turn("润色".into(), "润色".into(), vec![], None);
    harness.doc.ai_proposal = Some(AiProposal {
        before: before.clone(),
        result: GeneratedDraft {
            markdown: "# 标题\n\n润色后的原文。\n".into(),
            title: "标题".into(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_measured: false,
            files: Vec::new(),
        },
        label: "润色".into(),
        fact_changes: Vec::new(),
        excluded: Default::default(),
        fact_changes_confirmed: false,
        confirmed_facts: Vec::new(),
        view: Default::default(),
        open: false,
        locate: None,
    });
    harness
        .doc
        .ai_panel
        .finish(TurnState::Proposed(ProposalSummary::default()));
    harness.doc.ai_panel.composer.text = "请把现在的AI工作稿提交到正文".into();
    harness.with_page(|page| page.send_ai_panel());

    let composer = &harness.doc.ai_panel.composer;
    assert!(composer.commit_prompt, "弹出确认卡");
    assert!(composer.text.is_empty() && composer.error.is_none());
    assert_eq!(harness.doc.ai_panel.turns.len(), 1, "没有开新一轮");
    assert!(matches!(
        harness.doc.ai_panel.turns[0].state,
        TurnState::Proposed(_)
    ));
    assert!(harness.doc.ai_proposal.is_some(), "提案还挂着");
    assert_eq!(harness.doc.generated_markdown, before, "没点之前正文不动");
}

/// 没有待写入的提案时，说明原因，同样不发给模型。
#[test]
fn saying_commit_to_body_without_a_proposal_explains_why() {
    let mut harness = Harness::new("# 标题\n\n原文。\n");
    harness.doc.ai_panel.composer.text = "/采用".into();
    harness.with_page(|page| page.send_ai_panel());
    let composer = &harness.doc.ai_panel.composer;
    assert!(!composer.commit_prompt);
    assert!(
        composer
            .error
            .as_deref()
            .is_some_and(|error| error.contains("没有待写入的 AI 提案")),
        "{:?}",
        composer.error
    );
    assert!(harness.doc.ai_panel.turns.is_empty(), "没有发给技能");
}

/// 回归测试：跑任务时卡片里的阶段提示、过程记录很长，侧栏不能被撑宽、内容不能溢出侧栏。
///
/// 曾经阶段提示放在不换行的横排里，一长就把卡片撑得比侧栏还宽：侧栏被顶到最大宽度
/// 挤压正文区，卡片左右两头都被裁掉。
#[test]
fn long_phase_and_steps_stay_inside_the_panel() {
    let mut harness = Harness::new("# 研究报告\n\n正文内容。\n");
    theme::configure_style(&harness.ctx);
    harness.with_page(|page| page.toggle_ai_panel(None));
    let panel = &mut harness.doc.ai_panel;
    panel.push_turn("研究式起草".into(), "写一篇研究报告".into(), vec![], None);
    let turn = panel.turns.last_mut().unwrap();
    turn.state = TurnState::Streaming;
    let long = "第 2 轮 · 查「北约及盟国使用MSS时的数据共享边界、指挥权限、目标审批规则以及各类指挥员之间的授权、否决和复核流程」";
    turn.phase = long.into();
    turn.steps = (0..6)
        .map(|i| format!("{i}. 「北约成员国使用中的指挥关系、数据共享边界和目标审批规则以及独立评估机制、误差统计和事故复盘制度」没补上：证据回答不了"))
        .collect();
    let size = egui::vec2(1200.0, 900.0);
    let mut panel_rects = Vec::new();
    for _ in 0..20 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let ctx = harness.ctx.clone();
        let _ = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .show(ui, |ui| harness.with_page(|page| page.create_ui(ui)));
        });
        let state = harness.ctx.data(|data| {
            data.get_temp::<egui::containers::panel::PanelState>(egui::Id::new("ai_panel_v1"))
        });
        panel_rects.push(state.map(|state| state.outer_rect));
    }
    let first = panel_rects[1].expect("侧栏应有状态");
    let last = panel_rects.last().unwrap().expect("侧栏应有状态");
    assert!(
        (last.width() - first.width()).abs() < 1.0 && last.width() <= 421.0,
        "侧栏被内容撑宽：{panel_rects:?}"
    );
}
