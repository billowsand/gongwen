//! 决策：流程停下来问用户时，这一批题是什么形态、能不能让 AI 按要求重来
//! （`docs/decision-modules.md`）。
//!
//! 决策形态随挂起（`engine::Suspension`）交给界面、随会话保存：界面按形态画卡片，标题、占位与
//! 按钮文字取自形态，不再认「这一步是哪个算子」。回答怎么落到黑板仍由 `engine::apply_answers`
//! 按题目的 `Target` 走，确定性的。
//!
//! 修订（「让 AI 按要求重列」）的通用做法：界面把当前文本与修改要求作为修订请求写进挂起现场的
//! 黑板（[`REVISION`]），续跑位置拨回挂起的那一步；那一步见到修订请求就按要求重列、再挂起确认
//! （`ops::confirm`）。所以修订不用知道清单是谁产出的，恢复也仍然只有一个入口。

use super::engine::Suspension;
use super::skill::Skill;
use serde_json::json;

/// 修订请求存在黑板的这个变量里：`{"current": 当前文本, "instruction": 修改要求}`。
/// 名字沿用大纲时代的写法，旧检查点里挂着的修订请求照样认。
pub(crate) const REVISION: &str = "_outline_revision";

/// 挂起时这一批题的形态。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Decision {
    /// 一批选择题：定文种、动笔前、六要素、缺口、`ask.choice`。界面按题目的 `Target` 画。
    #[default]
    Choose,
    /// 确认一份清单（大纲、要点、检索问题……）：可直接改，可修订的还能让 AI 按要求重列。
    ConfirmList(ListConfirm),
}

impl Decision {
    pub(crate) fn list(&self) -> Option<&ListConfirm> {
        match self {
            Self::ConfirmList(list) => Some(list),
            Self::Choose => None,
        }
    }
}

/// 清单确认：卡片上的叫法与文字由产出清单的那一步给。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ListConfirm {
    /// 「大纲」「要点」「检索问题」。
    pub(crate) label: String,
    /// 确认后的清单存进哪个变量。
    pub(crate) var: String,
    /// 能不能让 AI 按修改要求重列。清单来自材料原文的不行，免得模型借机改事实。
    pub(crate) revisable: bool,
    /// 编辑框的占位。
    pub(crate) hint: String,
    /// 修改要求框的占位。
    pub(crate) instruction_hint: String,
    /// 确认按钮。
    pub(crate) submit: String,
    /// 勾选模式：逐条打勾取舍，勾中的项按原值存回；不能改字，也不能让 AI 重列（9.4）。
    #[serde(default)]
    pub(crate) pick: bool,
}

impl ListConfirm {
    pub(crate) fn new(label: &str, var: impl Into<String>, revisable: bool) -> Self {
        Self {
            label: label.to_string(),
            var: var.into(),
            revisable,
            hint: "每行一条".into(),
            instruction_hint: "例如：合并相近的几条，补上遗漏的，调整先后顺序".into(),
            submit: format!("确认{label}，继续"),
            pick: false,
        }
    }

    /// 勾选模式：每条一个勾选框、默认全勾。
    pub(crate) fn picking(label: &str, var: impl Into<String>) -> Self {
        Self {
            hint: "勾掉不要的".into(),
            instruction_hint: String::new(),
            submit: format!("按勾选的{label}继续"),
            pick: true,
            ..Self::new(label, var, false)
        }
    }

    /// 研究报告的大纲：每行一章，确认后开始逐章写。
    pub(crate) fn outline(var: impl Into<String>) -> Self {
        Self {
            hint: "每行一章：章标题：要点".into(),
            instruction_hint: "例如：合并前两章，增加国内外做法对比，把对策建议细分为三项".into(),
            submit: "确认大纲，开始写作".into(),
            ..Self::new("大纲", var, true)
        }
    }
}

/// 挂起的是可修订的清单确认时，返回挂起的那一步（修订时续跑位置拨回到它）。
/// 只认顶层的一步：`for_each` 里不能挂起，清单确认不会落在子流程中间。
pub(crate) fn revisable_step(suspension: &Suspension) -> Option<usize> {
    suspension.decision.list().filter(|list| list.revisable)?;
    let [next] = suspension.checkpoint.at.as_slice() else {
        return None;
    };
    next.checked_sub(1)
}

/// 用户在确认卡片上提了修改要求：写入修订请求，续跑位置拨回挂起的那一步。之后照常走
/// 唯一的恢复入口，那一步按要求重列后再停下来确认。
pub(crate) fn revise(
    suspension: &mut Suspension,
    current: &str,
    instruction: &str,
) -> Result<(), String> {
    let index = revisable_step(suspension).ok_or("当前不是可优化的清单确认。")?;
    if current.trim().is_empty() || instruction.trim().is_empty() {
        let label = suspension
            .decision
            .list()
            .map_or("清单", |list| list.label.as_str());
        return Err(format!("请保留{label}内容，并填写修改要求。"));
    }
    suspension.checkpoint.board.vars.insert(
        REVISION.into(),
        json!({"current": current.trim(), "instruction": instruction.trim()}),
    );
    suspension.checkpoint.at = vec![index];
    Ok(())
}

/// 旧会话里挂着的大纲确认没有存形态：按原来的认法（挂起在 `plan mode: outline` 之后、只有一道
/// 通用选择题、答案存进大纲变量）补成可修订的清单确认。其余照旧是一批选择题。
pub(crate) fn upgrade(skill: &Skill, suspension: &mut Suspension) {
    if suspension.decision != Decision::Choose {
        return;
    }
    let [next] = suspension.checkpoint.at.as_slice() else {
        return;
    };
    let Some(step) = next.checked_sub(1).and_then(|index| skill.flow.get(index)) else {
        return;
    };
    let name = step.save_as.as_deref().unwrap_or("outline");
    if step.step.as_deref() == Some("plan")
        && step.param_str("mode") == Some("outline")
        && suspension.save_as.as_deref() == Some(name)
        && suspension.questions.len() == 1
        && suspension.questions[0].target == super::clarify::Target::Pick
    {
        suspension.decision = Decision::ConfirmList(ListConfirm::outline(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::board::Board;
    use crate::agent::clarify::Reply;
    use crate::agent::testkit::{Driver, KeywordKb, ScriptedModel};

    fn skill() -> Skill {
        crate::agent::skill::parse("outline-test", "---\nname: 大纲测试\ntools: [ws.write]\nflow:\n  - step: plan\n    mode: outline\n    save_as: chapters\n  - tool: ws.write\n    args: { text: '{chapters}' }\n---\n## 大纲\n列出章的大纲：{request}\n", "测试").unwrap()
    }

    #[test]
    fn outline_can_be_revised_repeatedly_and_only_confirmation_continues() {
        let skill = skill();
        let model = ScriptedModel::new(|_, prompt| {
            if prompt.contains("【修改要求】\n增加风险分析") {
                assert!(prompt.contains("手工修改：保留这段"));
                "背景：手工修改\n风险：分析风险".into()
            } else if prompt.contains("【修改要求】\n合并为一章") {
                assert!(prompt.contains("风险：分析风险"));
                "综合分析：背景与风险".into()
            } else {
                "背景：原始大纲".into()
            }
        });
        let kb = KeywordKb::disabled();
        let original = Board {
            request: "分析人工智能应用".into(),
            ..Board::default()
        };
        let mut driver = Driver::new(&skill, &model, &kb, original.clone());
        let mut suspension = driver.run().expect("初次确认");
        assert_eq!(
            suspension.decision,
            Decision::ConfirmList(ListConfirm::outline("chapters"))
        );
        for (current, instruction, expected) in [
            (
                "手工修改：保留这段",
                "增加风险分析",
                "背景：手工修改\n风险：分析风险",
            ),
            (
                "背景：手工修改\n风险：分析风险",
                "合并为一章",
                "综合分析：背景与风险",
            ),
        ] {
            revise(&mut suspension, current, instruction).unwrap();
            // 优化请求现场可序列化，重开后仍携带手改文本和修改要求。
            let json = serde_json::to_string(&suspension.checkpoint).unwrap();
            let restored: crate::agent::checkpoint::Checkpoint =
                serde_json::from_str(&json).unwrap();
            assert_eq!(restored.board.vars[REVISION]["current"], current);
            driver.board = restored.board;
            suspension = driver.run_from(&restored.at).expect("优化完仍等确认");
            assert_eq!(suspension.questions[0].prefill, expected);
            assert_eq!(suspension.save_as.as_deref(), Some("chapters"));
            assert!(suspension.decision.list().is_some_and(|l| l.revisable));
            assert!(driver.board.workspace.is_empty(), "确认前不能进入写作");
            assert_eq!(driver.board.draft, original.draft);
        }
        driver.answer(
            &suspension,
            &[(1, Reply::Custom("最终手工定稿：采用这版".into()))],
        );
        assert!(driver.run().is_none());
        assert!(driver.board.workspace.contains("最终手工定稿：采用这版"));
        assert_eq!(model.asked("列出章的大纲"), 1, "优化不重做前序步骤");
    }

    #[test]
    fn a_model_connection_failure_returns_to_outline_confirmation() {
        struct FailedModel;
        impl crate::agent::backend::ModelBackend for FailedModel {
            fn complete(
                &self,
                _: crate::agent::backend::ModelRole,
                _: &str,
                _: &str,
                _: &mut dyn FnMut(crate::lmstudio::StreamDelta<'_>),
            ) -> anyhow::Result<crate::agent::backend::Completion> {
                anyhow::bail!("测试连接失败")
            }
            fn cancelled(&self) -> bool {
                false
            }
        }
        let skill = skill();
        let config = crate::models::AppConfig::default();
        let kb = KeywordKb::disabled();
        let manuscripts = crate::agent::tools::testing::FakeManuscripts { docs: vec![] };
        let apis = crate::agent::api::ApiStore::default();
        let secrets = crate::agent::api::ApiSecrets::default();
        let checkpoints = crate::agent::checkpoint::NoCheckpoint;
        let env = crate::agent::tools::Env {
            config: &config,
            vocabulary: &[],
            kb: &kb,
            manuscripts: &manuscripts,
            model: &FailedModel,
            skill: &skill,
            apis: &apis,
            secrets: &secrets,
            ckpt: &checkpoints,
        };
        let mut board = Board::default();
        board.vars.insert(
            REVISION.into(),
            json!({"current": "背景：手改大纲", "instruction": "加风险章"}),
        );
        let outcome = crate::agent::engine::run(&mut board, &env, &[0], &mut |_| {}).unwrap();
        let crate::agent::engine::Outcome::Suspended(suspension) = outcome else {
            panic!("连接失败仍应回到大纲确认");
        };
        assert_eq!(suspension.questions[0].prefill, "背景：手改大纲");
        assert!(board.workspace.is_empty());
    }

    #[test]
    fn empty_revision_keeps_the_edited_outline_and_validation_does_not_mutate_it() {
        let skill = skill();
        let model = ScriptedModel::new(|_, prompt| {
            if prompt.contains("【修改要求】") {
                "无".into()
            } else {
                "背景：原稿".into()
            }
        });
        let kb = KeywordKb::disabled();
        let mut driver = Driver::new(&skill, &model, &kb, Board::default());
        let mut suspension = driver.run().unwrap();
        assert_eq!(
            revise(&mut suspension, "背景：手工修改", " "),
            Err("请保留大纲内容，并填写修改要求。".into())
        );
        assert!(!suspension.checkpoint.board.vars.contains_key(REVISION));
        revise(&mut suspension, "背景：手工修改", "调整顺序").unwrap();
        driver.board = suspension.checkpoint.board;
        let next = driver.run_from(&suspension.checkpoint.at).unwrap();
        assert_eq!(next.questions[0].prefill, "背景：手工修改");
        assert!(driver.events.iter().any(|event| matches!(event,
            crate::agent::engine::Event::Note(text) if text.contains("已保留当前大纲"))));
        assert!(driver.board.workspace.is_empty());
    }

    #[test]
    fn confirm_step_can_confirm_and_revise_any_list_variable() {
        let skill = crate::agent::skill::parse(
            "confirm-test",
            "---\nname: 确认测试\ntools: [ws.write]\nflow:\n  - step: plan\n    mode: list\n    confirm: false\n    save_as: queries\n  - step: confirm\n    over: queries\n    label: 检索问题\n    revise: true\n  - tool: ws.write\n    args: { text: '问题：{queries}' }\n---\n## 清单\n列出要查的问题：{request}\n",
            "测试",
        )
        .unwrap();
        let model = ScriptedModel::new(|_, prompt| {
            if prompt.contains("【修改要求】\n加一条经费") {
                assert!(prompt.contains("【当前检索问题】\n防火责任"));
                "防火责任\n经费保障".into()
            } else {
                "防火责任\n检查安排".into()
            }
        });
        let kb = KeywordKb::disabled();
        let mut driver = Driver::new(&skill, &model, &kb, Board::default());
        let mut suspension = driver.run().expect("确认检索问题");
        let list = suspension.decision.list().expect("清单确认").clone();
        assert_eq!(
            (list.label.as_str(), list.var.as_str(), list.revisable),
            ("检索问题", "queries", true)
        );
        assert_eq!(list.submit, "确认检索问题，继续");
        assert_eq!(suspension.questions[0].prefill, "防火责任\n检查安排");
        assert_eq!(suspension.save_as.as_deref(), Some("queries"));

        revise(&mut suspension, "防火责任", "加一条经费").unwrap();
        assert_eq!(suspension.checkpoint.at, vec![1], "拨回确认这一步，不重列");
        driver.board = suspension.checkpoint.board.clone();
        let suspension = driver.run_from(&suspension.checkpoint.at).unwrap();
        assert_eq!(suspension.questions[0].prefill, "防火责任\n经费保障");
        assert_eq!(model.asked("列出要查的问题"), 1);

        // 点「就按这个写」：变量保持修订后的清单。
        driver.answer(&suspension, &[(1, Reply::Choice(0))]);
        assert!(driver.run().is_none());
        assert!(driver.board.workspace.contains("经费保障"));
    }

    #[test]
    fn confirm_step_is_not_revisable_by_default_and_skips_a_missing_list() {
        let skill = crate::agent::skill::parse(
            "confirm-test",
            "---\nname: 确认测试\ntools: [ws.write]\nflow:\n  - step: confirm\n    over: nothing\n  - step: plan\n    mode: split\n    confirm: false\n  - step: confirm\n    over: items\n    label: 要点\n---\n",
            "测试",
        )
        .unwrap();
        let model = ScriptedModel::new(|_, _| unreachable!("不调模型"));
        let kb = KeywordKb::disabled();
        let board = Board {
            request: "一、加强巡查。\n二、落实责任。".into(),
            ..Board::default()
        };
        let mut driver = Driver::new(&skill, &model, &kb, board);
        let mut suspension = driver.run().expect("确认要点");
        assert!(driver.events.iter().any(|event| matches!(event,
            crate::agent::engine::Event::Note(text) if text.contains("没有可确认的「nothing」"))));
        let list = suspension.decision.list().unwrap();
        assert!(!list.revisable);
        assert!(revise(&mut suspension, "一、加强巡查。", "合并").is_err());
    }

    #[test]
    fn ask_offers_the_material_value_for_a_conflicting_fact() {
        let skill = crate::agent::skill::parse(
            "conflict-test",
            "---\nname: 冲突测试\ntools: [ask.choice]\nflow:\n  - step: ask\n---\n",
            "测试",
        )
        .unwrap();
        let model = ScriptedModel::new(|_, _| "无".into());
        let kb = KeywordKb::disabled();
        let workspace = "截至9月底，全市共排查火灾隐患12项。";
        let mut board = Board {
            request: "截至9月底全市共排查火灾隐患15项。".into(),
            workspace: workspace.into(),
            ..Board::default()
        };
        board.ledger.sync(workspace, &board.request.clone(), &[]);
        for gap in &mut board.ledger.gaps {
            gap.status = crate::agent::gaps::GapStatus::NoAnswer;
        }
        let mut driver = Driver::new(&skill, &model, &kb, board);
        assert!(driver.run().is_none(), "默认附在提案上，不挂起");
        let question = &driver.board.questions[0];
        assert_eq!(question.choices[0].label, "按材料：15项");
        assert!(question.choices[0].recommended);
    }

    #[test]
    fn the_answer_decides_which_steps_run() {
        let text = "---\nname: 分支测试\ntools: [ask.choice, ws.write]\nflow:\n  - tool: ask.choice\n    args: { question: 怎么仿, options: [同类改稿, 结构仿写] }\n    save_as: plan\n  - tool: ws.write\n    args: { text: 改稿 }\n    when: { var: plan, eq: 同类改稿 }\n  - tool: ws.write\n    args: { text: 仿结构 }\n    when: { var: plan, in: [结构仿写, 整篇仿写] }\n---\n";
        let skill = crate::agent::skill::parse("branch-test", text, "测试").unwrap();
        let problems = crate::agent::skill::validate(
            &skill,
            &crate::agent::ops::names(),
            &crate::agent::tools::ids(),
        );
        assert!(problems.is_empty(), "{problems:?}");
        let model = ScriptedModel::new(|_, _| unreachable!("不调模型"));
        let kb = KeywordKb::disabled();
        let mut driver = Driver::new(&skill, &model, &kb, Board::default());
        let suspension = driver.run().expect("先问");
        driver.answer(&suspension, &[(1, Reply::Choice(1))]);
        assert!(driver.run().is_none());
        assert_eq!(driver.board.workspace, "仿结构", "只走了 in 对上的那一步");

        let bad = "---\nname: 坏\ntools: [ws.write]\nflow:\n  - tool: ws.write\n    args: { text: x }\n    when: [{ eq: 甲 }, { var: a, in: 甲 }, { var: b, eq: [甲, 乙] }]\n---\n";
        let skill = crate::agent::skill::parse("bad", bad, "测试").unwrap();
        let problems = crate::agent::skill::validate(
            &skill,
            &crate::agent::ops::names(),
            &crate::agent::tools::ids(),
        )
        .join("\n");
        assert!(problems.contains("eq 要和 var 一起写"), "{problems}");
        assert!(problems.contains("in 要写成列表"), "{problems}");
        assert!(problems.contains("eq 只能写一个值"), "{problems}");
    }

    #[test]
    fn picking_keeps_the_original_values_of_the_ticked_items() {
        let skill = crate::agent::skill::parse(
            "pick-test",
            "---\nname: 多选测试\ntools: [ws.write]\nflow:\n  - step: confirm\n    over: matters\n    label: 来函事项\n    pick: true\n---\n",
            "测试",
        )
        .unwrap();
        let model = ScriptedModel::new(|_, _| unreachable!("不调模型"));
        let kb = KeywordKb::disabled();
        let mut board = Board::default();
        board.vars.insert(
            "matters".into(),
            json!([{"id": 1, "title": "经费"}, {"id": 2, "title": "场地"}, {"id": 3, "title": "人员"}]),
        );
        let mut driver = Driver::new(&skill, &model, &kb, board);
        let suspension = driver.run().expect("逐条取舍");
        let list = suspension.decision.list().unwrap();
        assert!(list.pick && !list.revisable);
        assert_eq!(list.submit, "按勾选的来函事项继续");
        let question = &suspension.questions[0];
        assert_eq!(question.choices.len(), 3);
        assert!(question.choices.iter().all(|c| c.recommended), "默认全勾");
        driver.answer(&suspension, &[(1, Reply::Many(vec![0, 2]))]);
        assert!(driver.run().is_none());
        assert_eq!(
            driver.board.vars["matters"],
            json!([{"id": 1, "title": "经费"}, {"id": 3, "title": "人员"}]),
            "存的是原值，不是文字"
        );
    }

    #[test]
    fn evidence_is_picked_by_document_and_dropped_documents_stay_out() {
        use crate::agent::evidence::EvidenceDoc;
        let doc = |key: &str, title: &str| EvidenceDoc {
            key: key.into(),
            title: title.into(),
            section: String::new(),
            kind_label: "知识库".into(),
            text: format!("{title}的内容"),
        };
        let skill = crate::agent::skill::parse(
            "evidence-test",
            "---\nname: 证据测试\ntools: [ws.write]\nflow:\n  - step: pick_evidence\n  - tool: ws.write\n    args: { text: 写完 }\n---\n",
            "测试",
        )
        .unwrap();
        let model = ScriptedModel::new(|_, _| unreachable!("不调模型"));
        let kb = KeywordKb::disabled();
        let mut board = Board::default();
        board.evidence.absorb_docs(
            "检索",
            &[
                doc("kb:1", "森林防火条例"),
                doc("kb:2", "森林防火条例"),
                doc("kb:3", "2019年旧方案"),
                doc("kb:4", "应急预案"),
                doc("ref:9", "我引用的稿子"),
            ],
        );
        board.pinned = vec![5];
        let mut driver = Driver::new(&skill, &model, &kb, board);
        let suspension = driver.run().expect("三份文件就问");
        assert_eq!(suspension.checkpoint.at, vec![0], "答完回到这一步处理");
        let labels: Vec<&str> = suspension.questions[0]
            .choices
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "知识库《森林防火条例》（2 段）",
                "知识库《2019年旧方案》",
                "知识库《应急预案》"
            ],
            "同一份文件归成一条，@ 引用的不列"
        );
        driver.answer(&suspension, &[(1, Reply::Many(vec![0, 2]))]);
        assert!(driver.run().is_none());
        let titles: Vec<&str> = driver
            .board
            .evidence
            .items()
            .iter()
            .map(|e| e.doc_title.as_str())
            .collect();
        assert_eq!(
            titles,
            ["森林防火条例", "森林防火条例", "应急预案", "我引用的稿子"]
        );
        assert!(
            driver
                .tool_lines()
                .iter()
                .any(|l| l.contains("去掉 1 份资料"))
        );

        // 之后再检索到剔掉的文件（别的段落也算）不并入；新资料的编号不和已有的撞。
        let ids = driver.board.evidence.absorb_docs(
            "缺口检索",
            &[doc("kb:30", "2019年旧方案"), doc("kb:31", "新通知")],
        );
        assert_eq!(ids, [6]);
        assert!(
            !driver
                .board
                .evidence
                .items()
                .iter()
                .any(|e| e.doc_title == "2019年旧方案")
        );

        // 不到三份文件不问。
        let mut few = Board::default();
        few.evidence
            .absorb_docs("检索", &[doc("kb:1", "甲"), doc("kb:2", "乙")]);
        let mut driver = Driver::new(&skill, &model, &kb, few);
        assert!(driver.run().is_none());
    }

    #[test]
    fn evidence_ids_never_repeat_after_trimming() {
        use crate::agent::evidence::{EvidenceDoc, EvidencePack};
        let doc = |key: &str| EvidenceDoc {
            key: key.into(),
            title: key.into(),
            section: String::new(),
            kind_label: String::new(),
            text: String::new(),
        };
        let mut pack = EvidencePack::default();
        pack.absorb_docs("q", &[doc("a"), doc("b"), doc("c")]);
        pack.keep_last(1);
        assert_eq!(
            pack.absorb_docs("q", &[doc("d")]),
            [4],
            "原来会编成 2，和截掉的撞号"
        );
    }

    #[test]
    fn old_saved_outline_confirmations_are_upgraded() {
        let skill = skill();
        let model = ScriptedModel::new(|_, _| "背景：原始大纲".into());
        let kb = KeywordKb::disabled();
        let mut driver = Driver::new(&skill, &model, &kb, Board::default());
        let mut suspension = driver.run().unwrap();
        // 旧会话没存形态，读回是缺省值。
        suspension.decision = Decision::Choose;
        assert!(revisable_step(&suspension).is_none());
        upgrade(&skill, &mut suspension);
        assert_eq!(
            suspension.decision,
            Decision::ConfirmList(ListConfirm::outline("chapters"))
        );
        assert_eq!(revisable_step(&suspension), Some(0));

        // 别的选择题不动。
        suspension.decision = Decision::Choose;
        suspension.save_as = Some("other".into());
        upgrade(&skill, &mut suspension);
        assert_eq!(suspension.decision, Decision::Choose);
    }
}
