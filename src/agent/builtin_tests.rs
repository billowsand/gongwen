//! 内置技能的夹具测试：每个技能用按提示词回话的假模型、假知识库与假稿件库走一遍完整流程，
//! 挂起的地方像用户一样作答，检查每一步交给模型的东西和最后的工作稿、台账、题目。

use super::board::{Board, RefSource, Reference};
use super::clarify::{Reply, Target};
use super::engine::SkillReport;
use super::skill::{self, IMITATE, MATERIAL, POLICY_REPORT, REPLY_LETTER, Skill};
use super::testkit::{Driver, KeywordKb, ScriptedModel, chunk};
use super::tools::ManuscriptDoc;
use crate::agent::backend::ModelRole;
use crate::models::{DraftInput, ManuscriptStatus, TemplateKind};

fn builtin(id: &str) -> Skill {
    skill::builtin(id).expect("内置技能")
}

fn board(kind: TemplateKind, title: &str, request: &str) -> Board {
    Board {
        draft: DraftInput {
            kind,
            title_hint: title.into(),
            ..DraftInput::default()
        },
        request: request.into(),
        system_prompt: "SYSTEM".into(),
        time_sources: "2026年10月3日".into(),
        ..Board::default()
    }
}

#[test]
fn policy_report_confirms_the_outline_writes_by_chapter_and_cites_into_the_report() {
    let skill = builtin(POLICY_REPORT);
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("会让整篇写偏") {
            "无".into()
        } else if prompt.contains("列出章的大纲") {
            "1. 研究背景：为什么研究\n2. 对策建议：怎么办".into()
        } else if prompt.contains("里的一章：研究背景") {
            "## 研究背景\n\n梅文项目于2017年启动[K1]。".into()
        } else if prompt.contains("里的一章：国外做法") {
            "## 国外做法\n\n外军已部署智能辅助系统[K2]。系统已用于多个战区[K2]。".into()
        } else if prompt.contains("里的一章：对策建议") {
            "## 对策建议\n\n建议于【待核实：试点完成时限】前完成试点。".into()
        } else if prompt.contains("写一段摘要") {
            "本报告研究人工智能辅助决策的做法与对策。".into()
        } else if prompt.contains("具体事实（时间、数字") {
            "支持".into()
        } else if prompt.contains("需要补全") {
            "无法补全".into()
        } else {
            "无".into()
        }
    });
    let kb = KeywordKb::new(vec![
        (
            "研究背景",
            chunk(1, "梅文项目综述", "梅文项目于2017年启动。"),
        ),
        (
            "国外做法",
            chunk(2, "外军智能化报告", "外军已部署智能辅助系统。"),
        ),
    ]);
    let mut start = board(
        TemplateKind::ResearchReport,
        "人工智能辅助决策研究",
        "写一份决策参考，讲清背景、国外做法和对策",
    );
    start.draft.research.bibliography_content =
        "@book{maven,\n  title = {梅文项目综述},\n  author = {张三},\n  publisher = {某出版社},\n  year = {2024}\n}\n"
            .into();
    let mut driver = Driver::new(&skill, &model, &kb, start);

    // 列大纲后停下来，大纲预填在框里让用户改。
    let outline = driver.run().expect("大纲要你确认");
    let question = &outline.questions[0];
    assert_eq!(question.target, Target::Pick);
    assert_eq!(question.prefill, "研究背景：为什么研究\n对策建议：怎么办");
    assert_eq!(outline.save_as.as_deref(), Some("outline"));
    // 用户在框里加了一章。
    driver.answer(
        &outline,
        &[(
            1,
            Reply::Custom("研究背景：为什么研究\n国外做法：别人怎么做\n对策建议：怎么办".into()),
        )],
    );
    assert!(driver.run().is_none(), "确认大纲后一路写完");

    // 逐章写：每章只拿到这一章查到的证据，带着研究报告的写法规则。
    let chapters = model.prompts("里的一章：");
    assert_eq!(chapters.len(), 3);
    assert!(chapters[0].contains("--- [K1]") && !chapters[0].contains("--- [K2]"));
    assert!(chapters[1].contains("--- [K2]") && !chapters[1].contains("--- [K1]"));
    assert!(chapters[2].contains("没有查到可用的证据"));
    assert!(chapters.iter().all(|p| p.contains("文档类型为研究报告")));
    assert!(
        chapters[1].contains("梅文项目于2017年启动"),
        "后写的章能看到前面写好的部分"
    );

    let report = SkillReport::from_board(&driver.board);
    let text = &report.markdown;
    assert!(text.starts_with("# 人工智能辅助决策研究"), "{text}");
    assert!(
        text.contains("<!-- [摘要] -->\n\n本报告研究人工智能辅助决策的做法与对策。"),
        "摘要补进占位：{text}"
    );
    let order: Vec<usize> = ["## 研究背景", "## 国外做法", "## 对策建议"]
        .iter()
        .map(|heading| text.find(heading).expect(heading))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "按大纲顺序");
    // 引用落到报告：文献库里有的写成 [@key]，没有的写成脚注。
    assert!(text.contains("梅文项目于2017年启动[@maven]。"), "{text}");
    assert!(
        text.contains("外军已部署智能辅助系统[^k2]:(来源：《外军智能化报告》)。"),
        "{text}"
    );
    assert!(
        text.contains("系统已用于多个战区。"),
        "同一份资料在一章里只挂一次脚注：{text}"
    );
    assert!(!text.contains("[K"), "{text}");
    assert_eq!(
        model.asked("具体事实（时间、数字"),
        3,
        "三句带引用的话都核验过"
    );
    assert!(
        report
            .questions
            .iter()
            .any(|q| q.text.contains("试点完成时限"))
    );
    assert!(
        driver
            .tool_lines()
            .iter()
            .any(|l| l.contains("引用落到报告：文献库 1 处，脚注 1 处"))
    );
}

#[test]
fn imitate_picks_a_baseline_asks_what_changed_and_never_carries_old_facts_silently() {
    let skill = builtin(IMITATE);
    let model = ScriptedModel::new(|role, prompt| {
        if prompt.contains("旧稿（基准稿）") {
            "完成排查的时限｜沿用旧稿：11月30日｜另定".into()
        } else if role == ModelRole::Draft && prompt.contains("仿写方式") {
            "# 关于做好2026年冬季森林防火工作的通知\n\n各区县要于11月30日前完成隐患排查。\n".into()
        } else {
            "无".into()
        }
    });
    let kb = KeywordKb::disabled();
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        board(
            TemplateKind::PlainDocument,
            "",
            "仿照去年冬季森林防火通知，写今年的通知，排查改到12月1日前完成",
        ),
    );
    let doc = |id: i64, title: &str, markdown: &str| ManuscriptDoc {
        id,
        title: title.into(),
        kind: TemplateKind::PlainDocument,
        status: ManuscriptStatus::Published,
        doc_number: String::new(),
        doc_date: "2025-10-08".into(),
        draft: DraftInput::default(),
        markdown: markdown.into(),
        version: None,
    };
    driver.manuscripts.docs = vec![
        doc(
            2,
            "关于召开安全生产会议的通知",
            "# 关于召开安全生产会议的通知\n",
        ),
        doc(
            1,
            "关于做好2025年冬季森林防火工作的通知",
            "# 关于做好2025年冬季森林防火工作的通知\n\n各区县要于11月30日前完成隐患排查。\n",
        ),
    ];

    // ① 选基准稿：标题命中得多的排前面。
    let pick = driver.run().expect("要你选基准稿");
    assert_eq!(pick.questions[0].text, "照哪篇写？");
    assert!(
        pick.questions[0].choices[0]
            .label
            .contains("2025年冬季森林防火")
    );
    assert!(pick.questions[0].custom_hint.is_none(), "只能从候选里选");
    driver.answer(&pick, &[(1, Reply::Choice(0))]);
    assert_eq!(driver.board.vars["baseline_id"], 1);

    // ② 选仿写方式。
    let strategy = driver.run().expect("要你选怎么仿");
    assert_eq!(strategy.save_as.as_deref(), Some("strategy"));
    driver.answer(&strategy, &[(1, Reply::Choice(1))]);
    assert_eq!(driver.board.vars["strategy"], "结构仿写");

    // ③ 变化清单：对照旧稿问这次变了什么。
    let changes = driver.run().expect("要你确认变化");
    assert_eq!(changes.questions[0].target, Target::PreDraft);
    assert!(model.prompts("旧稿（基准稿）")[0].contains("11月30日前完成隐患排查"));
    driver.answer(&changes, &[(1, Reply::Custom("12月1日".into()))]);
    assert!(driver.run().is_none());

    let draft_prompt = &model.prompts("仿写方式")[0];
    assert!(
        draft_prompt.contains("【仿写方式：结构仿写】"),
        "{draft_prompt}"
    );
    assert!(
        draft_prompt.contains("只作写法参考") && draft_prompt.contains("11月30日前完成隐患排查")
    );
    assert!(
        draft_prompt.contains("完成排查的时限？12月1日"),
        "用户的回答作为已确认信息交给起草"
    );
    let report = SkillReport::from_board(&driver.board);
    assert!(report.evidence.is_empty(), "基准稿不进证据包");
    assert!(
        report.questions.iter().any(|q| q.text.contains("11月30日")),
        "照抄的旧日期被查出来问你：{:?}",
        report.questions.iter().map(|q| &q.text).collect::<Vec<_>>()
    );
}

#[test]
fn material_splits_points_without_a_model_and_writes_only_from_confirmed_points() {
    let skill = builtin(MATERIAL);
    assert!(!skill.uses_knowledge(), "材料成文只用材料，不检索");
    let model = ScriptedModel::new(|role, prompt| {
        if role == ModelRole::Draft && prompt.contains("已确认的要点") {
            "# 关于开展隐患排查的通知\n\n各区县于11月底前完成排查，共投入经费500万元。\n".into()
        } else {
            "无".into()
        }
    });
    let kb = KeywordKb::disabled();
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        board(
            TemplateKind::PlainDocument,
            "",
            "会议时间：10月10日\n- 各区县于11月底前完成排查\n- 市应急局牵头督导",
        ),
    );
    let points = driver.run().expect("要点要你确认");
    assert_eq!(
        model.calls.borrow().len(),
        2,
        "只调过动笔前澄清与六要素检查，拆要点不经模型"
    );
    assert_eq!(
        points.questions[0].prefill,
        "会议时间：10月10日\n各区县于11月底前完成排查\n市应急局牵头督导"
    );
    driver.answer(
        &points,
        &[(
            1,
            Reply::Custom("各区县于11月底前完成排查\n市应急局牵头督导".into()),
        )],
    );
    assert!(driver.run().is_none());
    let prompt = &model.prompts("已确认的要点")[0];
    assert!(
        prompt.contains("各区县于11月底前完成排查\n市应急局牵头督导"),
        "{prompt}"
    );
    let report = SkillReport::from_board(&driver.board);
    assert!(
        report.questions.iter().any(|q| q.text.contains("500万元")),
        "材料里没有的经费出题问你"
    );
    assert!(kb.queries.borrow().is_empty());
}

#[test]
fn reply_letter_lists_the_items_retrieves_basis_and_answers_each() {
    let skill = builtin(REPLY_LETTER);
    let model = ScriptedModel::new(|role, prompt| {
        if prompt.contains("需要逐项答复的事项") {
            "物资支援：请求支援帐篷20顶".into()
        } else if role == ModelRole::Draft && prompt.contains("来函事项——逐项答复") {
            "# 关于支援森林防火物资的复函\n\n你单位《关于商请支援森林防火物资的函》（林函〔2026〕5号）收悉。经研究，同意按规定调拨帐篷20顶[K1]。\n\n特此函复。\n".into()
        } else if prompt.contains("具体事实（时间、数字") {
            "支持".into()
        } else {
            "无".into()
        }
    });
    let kb = KeywordKb::new(vec![(
        "物资",
        chunk(
            1,
            "应急物资管理办法",
            "应急物资按规定调拨，帐篷20顶以内由本级审批。",
        ),
    )]);
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        board(
            TemplateKind::OfficialLetter,
            "",
            "来函：《关于商请支援森林防火物资的函》（林函〔2026〕5号），请求支援帐篷20顶。答复意见：同意支援。",
        ),
    );
    let items = driver.run().expect("来函事项要你确认");
    assert_eq!(items.questions[0].prefill, "物资支援：请求支援帐篷20顶");
    // 点「就按这个写」：清单保持原样。
    driver.answer(&items, &[(1, Reply::Choice(0))]);
    assert!(driver.board.vars["items"].is_array());
    assert!(driver.run().is_none());
    assert!(
        kb.queries.borrow().iter().any(|q| q.contains("物资支援")),
        "按事项检索依据"
    );
    let prompt = &model.prompts("来函事项——逐项答复")[0];
    assert!(prompt.contains("物资支援：请求支援帐篷20顶"));
    assert!(prompt.contains("[K1]"), "依据带编号交给起草");
    let report = SkillReport::from_board(&driver.board);
    assert!(report.markdown.contains("特此函复。"));
    assert!(!report.markdown.contains("[K"));
    assert_eq!(model.asked("具体事实（时间、数字"), 1);
    assert!(
        report.questions.is_empty(),
        "来函里的事实都有出处：{:?}",
        report.questions
    );
}

// —— 修改与审核类技能 ——

fn document_board(kind: TemplateKind, document: &str, request: &str) -> Board {
    let mut board = board(kind, "", request);
    board.document = document.into();
    board.workspace = document.into();
    board
}

#[test]
fn condense_measures_the_length_and_revises_once_when_far_off() {
    let skill = builtin(skill::CONDENSE);
    let long = "加强巡查。".repeat(40);
    let short = "加强巡查。".repeat(20);
    let replies = std::cell::RefCell::new(vec![long.clone(), short.clone()]);
    let model = ScriptedModel::new(move |_, _| replies.borrow_mut().remove(0));
    let kb = KeywordKb::disabled();
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        document_board(
            TemplateKind::PlainDocument,
            &"加强巡查。".repeat(60),
            "压缩到100字",
        ),
    );
    assert!(driver.run().is_none());
    let rewrites = model.prompts("");
    assert_eq!(rewrites.len(), 2, "第一次 200 字离 100 字太远，再改一次");
    assert!(
        rewrites[1].contains("现在约 200 字，要求约 100 字"),
        "{}",
        rewrites[1]
    );
    assert_eq!(driver.board.workspace, short);
    let lines = driver.tool_lines();
    assert!(lines.iter().any(|l| l.contains("再改一次")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("再改后字数 100")),
        "{lines:?}"
    );
}

#[test]
fn tone_rewrites_with_the_direction_rules_and_the_fact_lock() {
    let skill = builtin(skill::TONE);
    let model = ScriptedModel::new(|_, _| "# 关于申请经费的请示\n\n妥否，请批示。\n".into());
    let kb = KeywordKb::disabled();
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        document_board(
            TemplateKind::WhitePaper,
            "# 关于申请经费的请示\n\n速拨经费50万元。\n",
            "改成向市政府请示的上行文语气",
        ),
    );
    assert!(driver.run().is_none());
    let prompt = &model.prompts("")[0];
    assert!(prompt.contains("请示的结语用「妥否，请批示」") && prompt.contains("改成向市政府请示"));
    assert!(
        prompt.contains("50万元"),
        "事实锁定清单带上原文的数字：{prompt}"
    );
}

#[test]
fn normalize_replaces_aliases_first_then_asks_the_model_on_the_workspace() {
    let skill = builtin(skill::NORMALIZE);
    let model = ScriptedModel::new(|_, prompt| {
        assert!(
            prompt.contains("市林业和草原局要加强巡查"),
            "模型改的是换好名称的工作稿：{prompt}"
        );
        "市林业和草原局要加强巡查。".into()
    });
    let kb = KeywordKb::disabled();
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        document_board(
            TemplateKind::PlainDocument,
            "林草局要加强巡查。",
            "规范一下",
        ),
    );
    driver.vocabulary = vec![crate::models::VocabularyEntry {
        category: crate::models::VocabularyCategory::Unit,
        canonical: "市林业和草原局".into(),
        aliases: vec!["林草局".into()],
        ..Default::default()
    }];
    assert!(driver.run().is_none());
    assert!(
        driver
            .tool_lines()
            .iter()
            .any(|l| l.contains("规范化单位与人员名称 1 处"))
    );
    assert_eq!(driver.board.workspace, "市林业和草原局要加强巡查。");
}

#[test]
fn review_lists_problems_and_only_gated_exact_fixes_go_to_the_drawer() {
    let skill = builtin(skill::REVIEW);
    assert_eq!(skill.output, skill::OutputKind::Report);
    let document = "# 关于加强巡查的通知\n\n各地要加强巡查力度不断提高。\n\n请于12月1日前完成。\n";
    let model = ScriptedModel::new(|_, prompt| {
        assert!(prompt.contains("加强巡查力度不断提高"), "诊断要看到正文");
        [
            "表述｜搭配不当｜加强巡查力度不断提高｜不断加大巡查力度",
            "表述｜想改日期｜12月1日前完成｜12月5日前完成",
            "结构｜层次不清｜正文里没有这句话｜改成别的",
            "逻辑｜要求不明确｜请于12月1日前完成｜无",
        ]
        .join("\n")
    });
    let kb = KeywordKb::disabled();
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        document_board(TemplateKind::PlainDocument, document, "签发前审一下"),
    );
    assert!(driver.run().is_none());
    let findings = &driver.board.findings;
    let model_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.source.contains("模型诊断"))
        .collect();
    assert_eq!(model_findings.len(), 4);
    let fix = model_findings[0]
        .fix
        .as_ref()
        .expect("恰好出现一次、没动事实，收下改法");
    assert_eq!(&document[fix.span.clone()], "加强巡查力度不断提高");
    assert_eq!(fix.after, "不断加大巡查力度");
    assert!(
        model_findings[1].fix.is_none(),
        "改了日期，闸门拦下，只作提示"
    );
    assert!(model_findings[2].fix.is_none(), "原文找不到，不给改法");
    assert!(model_findings[3].fix.is_none(), "「无」表示只提示");
    assert_eq!(driver.board.workspace, document, "审核类不改稿");
}

#[test]
fn fact_check_marks_sourced_unsourced_and_contradicted_facts() {
    let skill = builtin(skill::FACT_CHECK);
    assert!(skill.uses_knowledge());
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("「500万元」") {
            "矛盾 K2：全年投入经费300万元".into()
        } else {
            "无关".into()
        }
    });
    let kb = KeywordKb::new(vec![
        (
            "森林火灾",
            chunk(1, "年度报告", "2025年全省共发生森林火灾12起，均已扑灭。"),
        ),
        ("经费", chunk(2, "财政决算", "全年投入经费300万元。")),
    ]);
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        document_board(
            TemplateKind::PlainDocument,
            "2025年全省共发生森林火灾12起。\n\n依据《森林防火条例》开展工作。\n\n全年投入经费500万元。\n",
            "核一下数字",
        ),
    );
    assert!(driver.run().is_none());
    let findings = &driver.board.findings;
    let group_of = |value: &str| {
        findings
            .iter()
            .find(|f| f.text.contains(value))
            .map(|f| (f.group.clone(), f.source.clone()))
            .unwrap_or_else(|| panic!("没有核「{value}」：{findings:?}"))
    };
    assert_eq!(findings[0].group, "与资料矛盾", "矛盾的排最前");
    let (group, source) = group_of("500万元");
    assert_eq!(group, "与资料矛盾");
    assert!(
        source.contains("[K2]") && source.contains("300万元"),
        "{source}"
    );
    assert_eq!(group_of("森林防火条例").0, "无出处");
    let (group, source) = group_of("2025年");
    assert_eq!(group, "有出处");
    assert!(source.contains("《年度报告》"), "{source}");
}

#[test]
fn extract_lists_points_without_touching_the_document() {
    let skill = builtin(skill::EXTRACT);
    let model = ScriptedModel::new(|_, prompt| {
        assert!(prompt.contains("（没有选区）"), "没有选区时提炼全文");
        "加强巡查\n落实责任".into()
    });
    let kb = KeywordKb::disabled();
    let document = "# 讲话\n\n要加强巡查，落实责任。\n";
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        document_board(TemplateKind::PlainDocument, document, "提炼要点"),
    );
    assert!(driver.run().is_none());
    let points: Vec<_> = driver
        .board
        .findings
        .iter()
        .map(|f| f.text.as_str())
        .collect();
    assert_eq!(points, ["加强巡查", "落实责任"]);
    assert!(driver.board.findings.iter().all(|f| f.group == "要点"));
    assert_eq!(driver.board.workspace, document);
}

/// 连真实模型、知识库与稿件库副本跑一个内置技能，挂起时按推荐项（或预填内容）作答。默认忽略；
/// 环境变量同 `engine::tests::live_research_draft`，另有：
/// - `GONGWEN_LIVE_SKILL`：技能 id（默认 `policy-report`）；
/// - `GONGWEN_LIVE_KIND`：文种名（默认按技能取）；`GONGWEN_LIVE_TITLE`：标题提示；
/// - `GONGWEN_LIVE_REQUEST`：用户原话；`GONGWEN_LIVE_DOCUMENT_FILE`：现有正文（修改、审核类技能用）；
/// - `GONGWEN_LIVE_STYLE_FILE`：一份风格档案（`StyleProfile` 的 JSON），像侧栏那样排进系统提示。
#[test]
#[ignore = "需要真实模型与知识库"]
fn live_builtin_skill() {
    use super::engine::{self, Event, Outcome};
    let env = |key: &str| std::env::var(key).unwrap_or_default();
    if env("GONGWEN_LIVE_LLM_URL").is_empty() || env("GONGWEN_LIVE_KB_DIR").is_empty() {
        eprintln!("未设置联机测试的环境变量，跳过");
        return;
    }
    crate::storage::set_test_config_dir(Some(env("GONGWEN_LIVE_KB_DIR").into()));
    let mut config = crate::models::AppConfig::default();
    config.lm_studio.base_url = env("GONGWEN_LIVE_LLM_URL");
    config.lm_studio.model = env("GONGWEN_LIVE_LLM_MODEL");
    config.lm_studio.api_key = env("GONGWEN_LIVE_LLM_KEY");
    config.lm_studio.timeout_seconds = 300;
    let mut rag = crate::models::RagConfig {
        enabled: true,
        ..Default::default()
    };
    rag.embedding.base_url = env("GONGWEN_LIVE_EMBED_URL");
    rag.embedding.model = env("GONGWEN_LIVE_EMBED_MODEL");
    rag.embedding.api_key = env("GONGWEN_LIVE_EMBED_KEY");
    rag.rerank.mode = crate::models::RerankMode::None;
    let id = match env("GONGWEN_LIVE_SKILL") {
        id if id.is_empty() => POLICY_REPORT.to_string(),
        id => id,
    };
    let skill = builtin(&id);
    let kind = TemplateKind::ALL
        .into_iter()
        .find(|kind| kind.label() == env("GONGWEN_LIVE_KIND"))
        .unwrap_or(match id.as_str() {
            POLICY_REPORT => TemplateKind::ResearchReport,
            REPLY_LETTER => TemplateKind::OfficialLetter,
            _ => TemplateKind::PlainDocument,
        });
    let kb = crate::agent::tools::RagSearch {
        enabled: skill.uses_knowledge(),
        rag,
        chat: config.lm_studio.clone(),
        kind_filter: None,
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = crate::agent::backend::LmBackend::new(&config, cancel);
    let time = crate::prompt::TimeContext::now();
    let mut board = board(
        kind,
        &env("GONGWEN_LIVE_TITLE"),
        &env("GONGWEN_LIVE_REQUEST"),
    );
    board.system_prompt = crate::prompt::build_system_prompt(&time);
    board.time_sources = format!("{} {}", time.today, time.now);
    {
        use crate::agent::backend::ModelBackend;
        eprintln!(
            "上下文窗口：{}",
            model
                .window(crate::agent::backend::ModelRole::Draft)
                .label()
        );
    }
    if let Ok(text) = std::fs::read_to_string(env("GONGWEN_LIVE_STYLE_FILE")) {
        let profile: crate::agent::style::StyleProfile =
            serde_json::from_str(&text).expect("风格档案 JSON");
        board.style = crate::agent::style::render(&profile, 6000);
        board.system_prompt = format!("{}\n\n{}", board.system_prompt, board.style);
        eprintln!("风格：{}", profile.name);
    }
    if let Ok(text) = std::fs::read_to_string(env("GONGWEN_LIVE_DOCUMENT_FILE")) {
        board.document = text.clone();
        board.workspace = text;
    }
    let apis = Default::default();
    let secrets = Default::default();
    let run_env = crate::agent::tools::Env {
        config: &config,
        vocabulary: &[],
        kb: &kb,
        manuscripts: &crate::agent::tools::SqliteManuscripts,
        model: &model,
        skill: &skill,
        apis: &apis,
        secrets: &secrets,
        ckpt: &crate::agent::checkpoint::NoCheckpoint,
    };
    let started = std::time::Instant::now();
    let mut print = |event: Event| match event {
        Event::Tool(tool) => eprintln!(
            "[{:>5.1}s] {}",
            started.elapsed().as_secs_f32(),
            tool.line()
        ),
        Event::Note(note) => eprintln!("        · {note}"),
        _ => {}
    };
    let mut next: Vec<usize> = Vec::new();
    loop {
        match engine::run(&mut board, &run_env, &next, &mut print).expect("技能应当跑通") {
            Outcome::Done => break,
            Outcome::Suspended(suspension) => {
                let replies: Vec<(usize, Reply)> = suspension
                    .questions
                    .iter()
                    .map(|q| {
                        eprintln!(
                            "  问：{} {:?}{}",
                            q.text,
                            q.choices
                                .iter()
                                .map(|c| c.label.as_str())
                                .collect::<Vec<_>>(),
                            if q.prefill.is_empty() {
                                String::new()
                            } else {
                                format!("\n{}", q.prefill)
                            }
                        );
                        let reply = if !q.prefill.is_empty() {
                            Reply::Custom(q.prefill.clone())
                        } else {
                            Reply::Choice(q.choices.iter().position(|c| c.recommended).unwrap_or(0))
                        };
                        (q.id, reply)
                    })
                    .collect();
                if let Some(kind) = engine::apply_answers(
                    &mut board,
                    &suspension.questions,
                    suspension.save_as.as_deref(),
                    &replies,
                ) {
                    board.draft.kind = kind;
                }
                next = suspension.checkpoint.at.clone();
            }
        }
    }
    let report = SkillReport::from_board(&board);
    eprintln!(
        "—— {}：证据 {} 段，{} 轮，用时 {:?} ——",
        skill.name,
        report.evidence.items().len(),
        report.rounds,
        started.elapsed()
    );
    for question in &report.questions {
        eprintln!("  题：{}", question.text);
    }
    for finding in &board.findings {
        eprintln!(
            "  [{}] {}{}｜{}｜{}",
            finding.group,
            finding.text,
            finding
                .fix
                .as_ref()
                .map(|fix| format!(" → 「{}」", fix.after))
                .unwrap_or_default(),
            finding.excerpt,
            finding.source
        );
    }
    if let Some(summary) = board.vars.get("agent_summary") {
        eprintln!("  自主步骤的答复：{summary}");
    }
    eprintln!("—— 工作稿 ——\n{}", report.markdown);
    if !skill.output.is_report(&board) {
        assert!(!report.markdown.trim().is_empty());
    }
}

fn reference(source: RefSource, id: i64, title: &str) -> Reference {
    Reference {
        source,
        id,
        title: title.into(),
    }
}

fn manuscript(id: i64, title: &str, markdown: &str) -> ManuscriptDoc {
    ManuscriptDoc {
        id,
        title: title.into(),
        kind: TemplateKind::PlainDocument,
        status: ManuscriptStatus::Published,
        doc_number: String::new(),
        doc_date: "2025-10-08".into(),
        draft: DraftInput::default(),
        markdown: markdown.into(),
        version: None,
    }
}

#[test]
fn imitate_takes_an_at_referenced_manuscript_as_the_baseline_without_asking() {
    let skill = builtin(IMITATE);
    let model = ScriptedModel::new(|_, _| "无".into());
    let kb = KeywordKb::disabled();
    let title = "关于做好2025年冬季森林防火工作的通知";
    let mut start = board(
        TemplateKind::PlainDocument,
        "",
        &format!("参照《{title}》写今年的，排查改到12月1日前完成"),
    );
    start.refs = vec![reference(RefSource::Manuscript, 1, title)];
    let mut driver = Driver::new(&skill, &model, &kb, start);
    driver.manuscripts.docs = vec![
        manuscript(
            2,
            "关于召开安全生产会议的通知",
            "# 关于召开安全生产会议的通知\n",
        ),
        manuscript(1, title, "# 通知\n\n各区县要于11月30日前完成隐患排查。\n"),
    ];
    let first = driver.run().expect("要你选怎么仿");
    assert_eq!(
        first.save_as.as_deref(),
        Some("strategy"),
        "@ 了基准稿就不再列候选让你选：{:?}",
        first.questions
    );
    assert_eq!(driver.board.vars["baseline_id"], 1);
    assert_eq!(driver.board.vars["baseline"]["title"], title);
    assert!(
        driver.board.evidence.is_empty(),
        "基准稿只作写法参考，不进证据包"
    );
    let lines = driver.tool_lines();
    assert!(
        lines.iter().any(|line| line.contains("当基准稿")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("读取稿件")),
        "不再按候选重读：{lines:?}"
    );
}

#[test]
fn reply_letter_takes_an_at_referenced_document_as_the_incoming_letter() {
    let skill = builtin(REPLY_LETTER);
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("需要逐项答复的事项") {
            "物资支援：请求支援帐篷20顶".into()
        } else {
            "无".into()
        }
    });
    let kb = KeywordKb::new(vec![(
        "不会命中",
        chunk(
            5,
            "关于商请支援森林防火物资的函",
            "请求支援帐篷20顶，请予支持为荷。",
        ),
    )]);
    let mut start = board(TemplateKind::OfficialLetter, "", "同意支援");
    start.refs = vec![reference(
        RefSource::Knowledge,
        5,
        "关于商请支援森林防火物资的函",
    )];
    let mut driver = Driver::new(&skill, &model, &kb, start);
    driver.run().expect("来函事项要你确认");
    let request = &driver.board.request;
    assert!(
        request.starts_with("【来函《关于商请支援森林防火物资的函》】\n请求支援帐篷20顶"),
        "{request}"
    );
    assert!(request.ends_with("【答复要求】\n同意支援"), "{request}");
    let asked = &model.prompts("需要逐项答复的事项")[0];
    assert!(
        asked.contains("请求支援帐篷20顶"),
        "列事项时看得到来函：{asked}"
    );
    assert!(driver.board.evidence.is_empty(), "来函不当证据");
}

#[test]
fn an_at_referenced_document_joins_the_evidence_and_stays_pinned() {
    let skill = builtin(POLICY_REPORT);
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("列出章的大纲") {
            "1. 研究背景：为什么研究".into()
        } else if prompt.contains("里的一章：研究背景") {
            "## 研究背景\n\n梅文项目于2017年启动[K1]。".into()
        } else {
            "无".into()
        }
    });
    // 知识库启用但检索什么也查不到：引用的那篇照样带进每一章。
    let kb = KeywordKb::new(Vec::new());
    let mut start = board(
        TemplateKind::ResearchReport,
        "梅文项目研究",
        "写一份研究报告",
    );
    start.refs = vec![reference(RefSource::Manuscript, 9, "梅文项目总结")];
    let mut driver = Driver::new(&skill, &model, &kb, start);
    driver.manuscripts.docs = vec![manuscript(
        9,
        "梅文项目总结",
        "梅文项目于2017年启动，2020年建成。",
    )];
    while let Some(suspension) = driver.run() {
        let replies: Vec<(usize, Reply)> = suspension
            .questions
            .iter()
            .map(|question| (question.id, Reply::Choice(0)))
            .collect();
        driver.answer(&suspension, &replies);
        if !model.prompts("里的一章：研究背景").is_empty() {
            break;
        }
    }
    let evidence = driver.board.evidence.items();
    assert_eq!(evidence.len(), 1, "{evidence:?}");
    assert_eq!(evidence[0].key, "ms:9:latest");
    assert_eq!(driver.board.pinned, [1]);
    let chapter = &model.prompts("里的一章：研究背景")[0];
    assert!(
        chapter.contains("[K1] 稿件库《梅文项目总结》"),
        "逐节检索没查到东西，引用的那篇也在：{chapter}"
    );
}

#[test]
fn material_reads_an_at_referenced_manuscript_and_skips_its_headers() {
    let skill = builtin(MATERIAL);
    let model = ScriptedModel::new(|_, _| "无".into());
    let kb = KeywordKb::disabled();
    let mut start = board(TemplateKind::PlainDocument, "", "整理成一份通知");
    start.refs = vec![reference(RefSource::Manuscript, 4, "10月10日会议纪要")];
    let mut driver = Driver::new(&skill, &model, &kb, start);
    driver.manuscripts.docs = vec![manuscript(
        4,
        "10月10日会议纪要",
        "# 10月10日会议纪要\n\n- 各区县于11月底前完成排查\n- 市应急局牵头督导\n",
    )];
    let points = driver.run().expect("要点要你确认");
    assert_eq!(
        points.questions[0].prefill, "各区县于11月底前完成排查\n市应急局牵头督导",
        "抬头、标题与要求都不算要点"
    );
}

#[test]
fn a_missing_reference_is_noted_and_skipped() {
    let skill = builtin(MATERIAL);
    let model = ScriptedModel::new(|_, _| "无".into());
    let kb = KeywordKb::disabled();
    let mut start = board(TemplateKind::PlainDocument, "", "会议决定：\n- 甲\n- 乙");
    start.refs = vec![reference(RefSource::Manuscript, 99, "已删除的稿件")];
    let mut driver = Driver::new(&skill, &model, &kb, start);
    driver.run().expect("照样往下走");
    assert!(
        driver.events.iter().any(|event| matches!(
            event,
            super::engine::Event::Note(note) if note.contains("《已删除的稿件》读不出来")
        )),
        "{:?}",
        driver.events
    );
    assert_eq!(driver.board.request, "会议决定：\n- 甲\n- 乙");
}

/// 风格学习（16.15 C.2）：`@` 的稿子当样稿交给算子，学出的档案存进变量、清单里列出写法描述，
/// 不进证据包；没有引用时说明要先 `@`。
#[test]
fn style_learn_reads_the_referenced_samples_and_proposes_a_profile() {
    let skill = builtin(skill::STYLE_LEARN);
    let model = ScriptedModel::new(|_, _| {
        "名称：对下部署类通知\n适用场合：部署、通知\n写法描述：\n总体基调：庄重。\n开头：为……现就有关事项通知如下。\n范例：1".into()
    });
    let kb = KeywordKb::disabled();
    let text = "为深入贯彻落实上级部署，切实做好今冬明春森林防火工作，现就有关事项通知如下。\n\n一、提高认识\n\n各地要压实责任、统筹推进，确保不发生重特大森林火灾。\n\n特此通知。";
    let mut start = board(TemplateKind::PlainDocument, "", "学一下这几篇的风格");
    start.refs = vec![
        reference(RefSource::Manuscript, 1, "防火通知"),
        reference(RefSource::Manuscript, 2, "安全通知"),
    ];
    let mut driver = Driver::new(&skill, &model, &kb, start);
    driver.manuscripts.docs = vec![
        manuscript(1, "防火通知", text),
        manuscript(2, "安全通知", text),
    ];
    assert!(driver.run().is_none());
    let profile: crate::agent::style::StyleProfile =
        serde_json::from_value(driver.board.vars[crate::agent::ops::STYLE_PROFILE].clone())
            .unwrap();
    assert_eq!(profile.name, "对下部署类通知");
    assert_eq!(profile.sources.len(), 2);
    assert!(driver.board.evidence.is_empty(), "样稿不进证据包");
    let groups: Vec<&str> = driver
        .board
        .findings
        .iter()
        .map(|f| f.group.as_str())
        .collect();
    assert_eq!(groups[0], "风格档案");
    assert!(groups.contains(&"写法描述"));
    assert!(skill.output.is_report(&driver.board));

    // 没有 @ 引用：说清要先引用。
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        board(TemplateKind::PlainDocument, "", "学一下风格"),
    );
    let error = driver.try_run().unwrap_err().to_string();
    assert!(error.contains("先用 @ 引用"), "{error}");
}

#[test]
fn letters_ask_the_six_elements_before_drafting_and_write_the_answers_in() {
    let skill = builtin(skill::RESEARCH_DRAFT);
    let model = ScriptedModel::new(|role, prompt| {
        if prompt.contains("逐项检查起草所需的六要素") {
            "何事｜已给｜商请共建公共数据研究平台\n\
             何因｜已给｜根据市政府常务会议要求\n\
             何人｜缺｜我方联系人是谁？\n\
             何时｜缺｜请对方什么时候前反馈？\n\
             何地｜不适用\n\
             何法｜已给｜请书面函复"
                .into()
        } else if role == ModelRole::Draft && prompt.contains("研究平台") {
            "# 关于商请共建公共数据研究平台的函\n\n市数据局：\n\n根据市政府常务会议要求，商请共建\
             公共数据研究平台。请于10月31日前书面函复。联系人：【待核实：联系人及电话】。\n"
                .into()
        } else {
            "无".into()
        }
    });
    let kb = KeywordKb::disabled();
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        board(
            TemplateKind::OfficialLetter,
            "",
            "给市数据局发函，商请共建公共数据研究平台。根据市政府常务会议要求。请书面函复。",
        ),
    );
    let asked = driver.run().expect("动笔前要问六要素");
    let targets: Vec<_> = asked.questions.iter().map(|q| q.target).collect();
    assert_eq!(
        targets,
        [
            Target::Element(super::elements::Element::Who),
            Target::Element(super::elements::Element::When),
        ]
    );
    driver.answer(
        &asked,
        &[(1, Reply::Skip), (2, Reply::Custom("10月31日前".into()))],
    );
    assert!(driver.run().is_none());
    let draft_prompt = &model
        .prompts("研究平台")
        .last()
        .cloned()
        .unwrap_or_default();
    assert!(
        draft_prompt.contains("回复时限（请对方办理或回复的时限）：10月31日前（起草人确认）"),
        "{draft_prompt}"
    );
    assert!(draft_prompt.contains("【待核实：联系人及电话】"));
    let report = SkillReport::from_board(&driver.board);
    assert!(
        report.questions.is_empty(),
        "答过的日期有出处，选了先不定的不再问：{:?}",
        report.questions.iter().map(|q| &q.text).collect::<Vec<_>>()
    );
}

#[test]
fn retrievable_gaps_the_knowledge_base_cannot_fill_are_generalized_not_asked() {
    let skill = builtin(skill::RESEARCH_DRAFT);
    let model = ScriptedModel::new(|role, prompt| {
        if prompt.contains("需要补全") {
            "无法补全".into()
        } else if prompt.contains("材料和知识库里都查不到") {
            "依据有关规定，现就开展隐患排查有关事项通知如下。".into()
        } else if role == ModelRole::Draft && prompt.contains("隐患排查") {
            "# 关于开展冬季森林防火隐患排查的通知\n\n各区县：\n\n\
             依据【待核实：上级文件依据】，现就开展隐患排查有关事项通知如下。\n"
                .into()
        } else {
            "无".into()
        }
    });
    let kb = KeywordKb::new(vec![(
        "隐患排查",
        chunk(1, "往年通知", "各地要认真开展隐患排查。"),
    )]);
    let mut driver = Driver::new(
        &skill,
        &model,
        &kb,
        board(
            TemplateKind::PlainDocument,
            "",
            "起草一份通知，部署冬季森林防火隐患排查。",
        ),
    );
    assert!(driver.run().is_none());
    assert!(
        driver
            .board
            .workspace
            .contains("依据有关规定，现就开展隐患排查有关事项通知如下。"),
        "{}",
        driver.board.workspace
    );
    let gap = &driver.board.ledger.gaps[0];
    assert_eq!(
        gap.status,
        super::gaps::GapStatus::Generalized(
            "依据【待核实：上级文件依据】，现就开展隐患排查有关事项通知如下。".into()
        )
    );
    let report = SkillReport::from_board(&driver.board);
    assert!(report.questions.is_empty(), "概括过的不再问");
}

/// 连真实模型与真实 SQLite 跑一遍检查点（内核加固第 4 期联机验收）：
/// 跑到一半停掉（模拟崩溃），从库里的检查点接着跑完，产出工作稿。
///
/// 环境变量同 [`live_builtin_skill`]。`GONGWEN_LIVE_STOP_AFTER`：在前 N 份检查点之后
/// 主动放弃本次运行（模拟进程被杀），再从最新一份恢复。
#[test]
#[ignore = "需要真实模型与知识库"]
fn live_checkpoint_resume_after_an_interruption() {
    use super::checkpoint::{Checkpoint, CheckpointSink, Reason};
    use super::engine::{self, Event, Outcome};
    use crate::manuscript::ai_sessions::AiSessionRecord;
    use crate::manuscript::{ManuscriptStore, NewManuscript};

    let env = |key: &str| std::env::var(key).unwrap_or_default();
    if env("GONGWEN_LIVE_LLM_URL").is_empty() || env("GONGWEN_LIVE_KB_DIR").is_empty() {
        eprintln!("未设置联机测试的环境变量，跳过");
        return;
    }
    crate::storage::set_test_config_dir(Some(env("GONGWEN_LIVE_KB_DIR").into()));
    let mut config = crate::models::AppConfig::default();
    config.lm_studio.base_url = env("GONGWEN_LIVE_LLM_URL");
    config.lm_studio.model = env("GONGWEN_LIVE_LLM_MODEL");
    config.lm_studio.api_key = env("GONGWEN_LIVE_LLM_KEY");
    config.lm_studio.timeout_seconds = 300;
    let rag = crate::models::RagConfig {
        enabled: true,
        embedding: crate::models::EmbeddingConfig {
            base_url: env("GONGWEN_LIVE_EMBED_URL"),
            model: env("GONGWEN_LIVE_EMBED_MODEL"),
            api_key: env("GONGWEN_LIVE_EMBED_KEY"),
            ..Default::default()
        },
        rerank: crate::models::RerankConfig {
            mode: crate::models::RerankMode::None,
            ..Default::default()
        },
        ..Default::default()
    };
    let kb = crate::agent::tools::RagSearch {
        enabled: true,
        rag: rag.clone(),
        chat: config.lm_studio.clone(),
        kind_filter: None,
    };

    // 稿件、会话与这一轮先入库：检查点的外键挂在轮次上。
    let db = crate::storage::manuscript_db_path().expect("稿件库路径");
    let session_id = "live-ckpt-session".to_string();
    {
        let mut store = ManuscriptStore::open(&db).expect("打开稿件库");
        let manuscript = store
            .create(&NewManuscript::default(), None)
            .expect("建稿件");
        let record = AiSessionRecord {
            id: session_id.clone(),
            title: "联机检查点".into(),
            is_current: true,
            created_at: String::new(),
            updated_at: String::new(),
            ..AiSessionRecord::default()
        };
        store
            .save_ai_session(manuscript, &record, &[(1, "{}".into())], &[1])
            .expect("存会话与轮次");
    }

    let skill = builtin(POLICY_REPORT);
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = crate::agent::backend::LmBackend::new(&config, cancel);
    let time = crate::prompt::TimeContext::now();
    let mut board = board(
        TemplateKind::ResearchReport,
        &env("GONGWEN_LIVE_TITLE"),
        &env("GONGWEN_LIVE_REQUEST"),
    );
    board.system_prompt = crate::prompt::build_system_prompt(&time);
    board.time_sources = format!("{} {}", time.today, time.now);

    let apis = Default::default();
    let secrets = Default::default();
    let started = std::time::Instant::now();
    let mut print = |event: Event| match event {
        Event::Tool(tool) => eprintln!(
            "[{:>5.1}s] {}",
            started.elapsed().as_secs_f32(),
            tool.line()
        ),
        Event::Note(note) => eprintln!("        · {note}"),
        _ => {}
    };

    // 第一遍：跑到第 N 份检查点就「崩」（放弃本次运行），检查点已经在库里。
    let stop_after: usize = env("GONGWEN_LIVE_STOP_AFTER").parse().unwrap_or(3);
    let sink = |ckpt: &Checkpoint| -> Result<(), String> {
        let mut store = ManuscriptStore::open(&db).map_err(|e| format!("{e:#}"))?;
        store
            .save_run_checkpoint(
                &session_id,
                1,
                &skill.id,
                &crate::ai_panel::session::skill_hash(&skill),
                true,
                ckpt,
            )
            .map_err(|e| format!("{e:#}"))
    };
    struct CountSink<F: Fn(&Checkpoint) -> Result<(), String>> {
        inner: F,
        saved: std::cell::RefCell<Vec<Vec<usize>>>,
    }
    impl<F: Fn(&Checkpoint) -> Result<(), String>> CheckpointSink for CountSink<F> {
        fn save(&self, ckpt: &Checkpoint) -> Result<(), String> {
            (self.inner)(ckpt)?;
            self.saved.borrow_mut().push(ckpt.at.clone());
            Ok(())
        }
    }
    let count = CountSink {
        inner: sink,
        saved: std::cell::RefCell::new(Vec::new()),
    };
    let no_sink = crate::agent::checkpoint::NoCheckpoint;
    struct OnlyFirst<'a> {
        inner: &'a dyn CheckpointSink,
        left: std::cell::Cell<usize>,
    }
    impl CheckpointSink for OnlyFirst<'_> {
        fn save(&self, ckpt: &Checkpoint) -> Result<(), String> {
            if self.left.get() == 0 {
                return Ok(());
            }
            self.left.set(self.left.get() - 1);
            self.inner.save(ckpt)
        }
    }
    let only_first = OnlyFirst {
        inner: &count,
        left: std::cell::Cell::new(stop_after),
    };
    let ckpt_env = crate::agent::tools::Env {
        config: &config,
        vocabulary: &[],
        kb: &kb,
        manuscripts: &crate::agent::tools::SqliteManuscripts,
        model: &model,
        skill: &skill,
        apis: &apis,
        secrets: &secrets,
        ckpt: &only_first,
    };
    let outcome = engine::run(&mut board, &ckpt_env, &[], &mut print);
    eprintln!(
        "—— 第一遍 {:?} 后放弃（模拟崩溃）：{outcome:?}",
        started.elapsed()
    );

    // 从库里取最新一份检查点，接着跑。
    let store = ManuscriptStore::open(&db).expect("打开稿件库");
    let latest = store
        .latest_run_checkpoint(&session_id, 1)
        .expect("读检查点")
        .expect("应当有检查点");
    eprintln!(
        "检查点 {} 份；恢复位置 at={:?}（{}）",
        store
            .list_run_checkpoints(&session_id, 1)
            .map(|list| list.len())
            .unwrap_or(0),
        latest.checkpoint.at,
        latest.label
    );
    assert!(
        matches!(latest.checkpoint.reason, Reason::Step | Reason::Ask),
        "读回来的检查点要能续跑，实际 {:?}",
        latest.checkpoint.reason
    );
    assert!(!count.saved.borrow().is_empty(), "第一遍至少落了一份检查点");

    // 只读区按界面当前值重灌（红线 2），再从检查点接着跑。
    let mut resumed = latest.checkpoint.board;
    let at = latest.checkpoint.at.clone();
    // 检索配置与第一遍相同（embedding 那几项第一遍是单独填的）。
    let kb2 = crate::agent::tools::RagSearch {
        enabled: true,
        rag: rag.clone(),
        chat: config.draft_chat().unwrap_or_default(),
        kind_filter: None,
    };
    let resume_env = crate::agent::tools::Env {
        config: &config,
        vocabulary: &[],
        kb: &kb2,
        manuscripts: &crate::agent::tools::SqliteManuscripts,
        model: &model,
        skill: &skill,
        apis: &apis,
        secrets: &secrets,
        ckpt: &no_sink,
    };
    // 有题就照第一遍那样作答，直到跑完。
    let mut at = at;
    loop {
        match engine::run(&mut resumed, &resume_env, &at, &mut print).expect("恢复应当跑通") {
            Outcome::Done => break,
            Outcome::Suspended(suspension) => {
                let replies: Vec<(usize, Reply)> = suspension
                    .questions
                    .iter()
                    .map(|q| {
                        eprintln!("  问：{}", q.text);
                        (q.id, Reply::Choice(0))
                    })
                    .collect();
                let kind = engine::apply_answers(
                    &mut resumed,
                    &suspension.questions,
                    suspension.save_as.as_deref(),
                    &replies,
                );
                if let Some(kind) = kind {
                    resumed.draft.kind = kind;
                }
                at = suspension.checkpoint.at.clone();
            }
        }
    }
    let report = SkillReport::from_board(&resumed);
    eprintln!(
        "—— 恢复跑完 {:?}：工作稿 {} 字，证据 {} 段，题目 {} 道 ——",
        started.elapsed(),
        report.markdown.chars().count(),
        report.evidence.items().len(),
        report.questions.len()
    );
    assert!(!report.markdown.trim().is_empty(), "恢复后照样出稿");
}
