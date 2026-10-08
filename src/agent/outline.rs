//! 大纲确认期间的反复优化：保留挂起位置，只重跑列大纲那一步。

use super::engine::Suspension;
use super::skill::Skill;
use serde_json::json;

pub(crate) const REVISION: &str = "_outline_revision";

/// 只识别真正的大纲确认，不把其它通用选择题当作大纲。
pub(crate) fn step_index(skill: &Skill, suspension: &Suspension) -> Option<usize> {
    let [next] = suspension.checkpoint.at.as_slice() else {
        return None;
    };
    let index = next.checked_sub(1)?;
    let step = skill.flow.get(index)?;
    let name = step.save_as.as_deref().unwrap_or("outline");
    (step.step.as_deref() == Some("plan")
        && step.param_str("mode") == Some("outline")
        && suspension.save_as.as_deref() == Some(name)
        && suspension.questions.len() == 1
        && suspension.questions[0].target == super::clarify::Target::Pick)
        .then_some(index)
}

pub(crate) fn revise(
    skill: &Skill,
    suspension: &mut Suspension,
    current: &str,
    instruction: &str,
) -> Result<(), String> {
    let index = step_index(skill, suspension).ok_or("当前不是可优化的大纲确认步骤。")?;
    if current.trim().is_empty() || instruction.trim().is_empty() {
        return Err("请保留大纲内容，并填写修改要求。".into());
    }
    suspension.checkpoint.board.vars.insert(
        REVISION.into(),
        json!({"current": current.trim(), "instruction": instruction.trim()}),
    );
    suspension.checkpoint.at = vec![index];
    Ok(())
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
            revise(&skill, &mut suspension, current, instruction).unwrap();
            // 优化请求现场可序列化，重开后仍携带手改文本和修改要求。
            let json = serde_json::to_string(&suspension.checkpoint).unwrap();
            let restored: crate::agent::checkpoint::Checkpoint =
                serde_json::from_str(&json).unwrap();
            assert_eq!(restored.board.vars[REVISION]["current"], current);
            driver.board = restored.board;
            suspension = driver.run_from(&restored.at).expect("优化完仍等确认");
            assert_eq!(suspension.questions[0].prefill, expected);
            assert_eq!(suspension.save_as.as_deref(), Some("chapters"));
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
        assert!(revise(&skill, &mut suspension, "背景：手工修改", " ").is_err());
        assert!(!suspension.checkpoint.board.vars.contains_key(REVISION));
        revise(&skill, &mut suspension, "背景：手工修改", "调整顺序").unwrap();
        driver.board = suspension.checkpoint.board;
        let next = driver.run_from(&suspension.checkpoint.at).unwrap();
        assert_eq!(next.questions[0].prefill, "背景：手工修改");
        assert!(driver.events.iter().any(|event| matches!(event,
            crate::agent::engine::Event::Note(text) if text.contains("已保留当前大纲"))));
        assert!(driver.board.workspace.is_empty());
    }
}
