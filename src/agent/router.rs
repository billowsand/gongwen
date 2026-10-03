//! 技能选择（`docs/ai-agent-workbench.md` 16.7）：
//!
//! 1. 显式：输入框以「/技能名」开头，或用 `/` 弹出列表、技能标签选定；
//! 2. 上下文过滤：停用的、`applies_to` 与 `when` 不满足的不参与；
//! 3. 触发词打分：命中的触发词越长分越高，第一名领先就选它；
//! 4. 模型判断：分不出高下时，发送后让辅助模型从候选的 `description` 里选一个（封闭集）。

use super::backend::{ModelBackend, ModelRole};
use super::skill::{SelectionNeed, Skill, TextNeed};
use super::tools::ASSIST_SYSTEM;
use crate::models::TemplateKind;
use regex::Regex;

/// 选技能时看的上下文。
#[derive(Debug, Clone, Copy)]
pub(crate) struct RouteContext<'a> {
    pub(crate) text: &'a str,
    pub(crate) has_text: bool,
    pub(crate) has_selection: bool,
    pub(crate) kind: TemplateKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    /// 选定了（显式或打分胜出）。
    Picked(String),
    /// 分不出高下，交模型从这几个里选。
    Ambiguous(Vec<String>),
    /// 当前情况下没有可用的技能。
    Nothing,
}

/// 这个技能在当前上下文里能不能用。
pub(crate) fn eligible(skill: &Skill, ctx: &RouteContext<'_>) -> bool {
    let text_ok = match skill.when.text {
        TextNeed::Any => true,
        TextNeed::Empty => !ctx.has_text,
        TextNeed::Present => ctx.has_text,
    };
    let selection_ok = match skill.when.selection {
        SelectionNeed::Any => true,
        SelectionNeed::Required => ctx.has_selection,
        SelectionNeed::None => !ctx.has_selection,
    };
    skill.enabled
        && text_ok
        && selection_ok
        && (skill.applies_to.is_empty() || skill.applies_to.contains(&ctx.kind))
}

/// 触发词得分：每个命中的触发词按字数计分。含正则符号的触发词按正则匹配
/// （如「按照.*的格式」），写坏了的正则当作没命中。
pub(crate) fn score(skill: &Skill, text: &str) -> usize {
    skill
        .triggers
        .iter()
        .filter(|trigger| !trigger.trim().is_empty())
        .filter_map(|trigger| {
            if trigger.contains(['.', '*', '+', '?', '[', '(', '|', '^', '$']) {
                Regex::new(trigger)
                    .ok()?
                    .find(text)
                    .map(|m| m.as_str().chars().count())
            } else {
                text.contains(trigger.as_str())
                    .then(|| trigger.chars().count())
            }
        })
        .sum()
}

/// 「/润色 压缩一下」→ (技能 id, 「压缩一下」)。按技能名或 id 认，认不出返回 None。
pub(crate) fn slash<'t>(skills: &[Skill], text: &'t str) -> Option<(String, &'t str)> {
    let rest = text.trim_start().strip_prefix(['/', '／'])?;
    let (word, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    skills
        .iter()
        .find(|skill| skill.enabled && (skill.name == word || skill.id == word))
        .map(|skill| (skill.id.clone(), tail.trim_start()))
}

/// 按上下文与触发词选技能。
pub(crate) fn route(skills: &[Skill], ctx: &RouteContext<'_>) -> Route {
    let candidates: Vec<&Skill> = skills.iter().filter(|s| eligible(s, ctx)).collect();
    match candidates.as_slice() {
        [] => return Route::Nothing,
        [only] => return Route::Picked(only.id.clone()),
        _ => {}
    }
    let mut scored: Vec<(usize, &Skill)> = candidates
        .iter()
        .map(|skill| (score(skill, ctx.text), *skill))
        .collect();
    scored.sort_by_key(|(points, _)| std::cmp::Reverse(*points));
    let top = scored[0].0;
    if top > 0 && scored[1].0 < top {
        return Route::Picked(scored[0].1.id.clone());
    }
    // 第一名不唯一（含全都没命中）：并列的交模型选。
    Route::Ambiguous(
        scored
            .iter()
            .take_while(|(points, _)| *points == top)
            .map(|(_, skill)| skill.id.clone())
            .collect(),
    )
}

/// 模型判断：从候选里选一个，返回下标。模型答非所问时退回第一个候选。
pub(crate) fn choose_by_model(
    model: &dyn ModelBackend,
    candidates: &[&Skill],
    request: &str,
) -> anyhow::Result<usize> {
    if candidates.len() <= 1 {
        return Ok(0);
    }
    let list = candidates
        .iter()
        .enumerate()
        .map(|(index, skill)| format!("{}. {}：{}", index + 1, skill.name, skill.description))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "用户在公文写作软件里提出了下面的要求，判断该用哪个技能处理。\n\n【可用技能】\n{list}\n\n\
         【用户要求】\n{}\n\n只输出技能编号（一个数字）。",
        request.trim()
    );
    let reply = model
        .complete(ModelRole::Assist, ASSIST_SYSTEM, &prompt, &mut |_| {})?
        .content;
    let picked = reply
        .split(|c: char| !c.is_ascii_digit())
        .find_map(|digits| digits.parse::<usize>().ok())
        .filter(|n| (1..=candidates.len()).contains(n))
        .map_or(0, |n| n - 1);
    Ok(picked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::backend::Completion;
    use crate::agent::skill::{
        CONDENSE, EXTRACT, FACT_CHECK, IMITATE, MATERIAL, NORMALIZE, POLICY_REPORT, POLISH,
        REPLY_LETTER, RESEARCH_DRAFT, REVIEW, TONE, builtin_skills, parse,
    };
    use crate::lmstudio::StreamDelta;

    fn ctx(text: &str, has_text: bool) -> RouteContext<'_> {
        RouteContext {
            text,
            has_text,
            has_selection: false,
            kind: TemplateKind::PlainDocument,
        }
    }

    #[test]
    fn an_empty_document_rules_out_polishing() {
        let skills = builtin_skills();
        let Route::Ambiguous(tied) = route(&skills, &ctx("润色一下", false)) else {
            panic!("空稿上「润色」不命中任何能用的技能，交模型");
        };
        assert!(
            !tied.contains(&POLISH.to_string()),
            "正文为空时润色不参与：{tied:?}"
        );
        assert!(tied.contains(&RESEARCH_DRAFT.to_string()));
    }

    #[test]
    fn triggers_pick_the_skill_and_ties_go_to_the_model() {
        let skills = builtin_skills();
        let picked = |text: &str, has_text: bool| route(&skills, &ctx(text, has_text));
        for (text, id) in [
            ("压缩到800字，不改时限", CONDENSE),
            ("润色一下第二段", POLISH),
            ("改成向市政府请示的上行文语气", TONE),
            ("把单位名称规范一下", NORMALIZE),
            ("签发前帮我审一下", REVIEW),
            ("核实一下文中的数字", FACT_CHECK),
            ("提炼这篇讲话的要点", EXTRACT),
        ] {
            assert_eq!(picked(text, true), Route::Picked(id.into()), "{text}");
        }
        assert_eq!(
            picked("起草一份关于冬季森林防火的通知", false),
            Route::Picked(RESEARCH_DRAFT.into())
        );
        assert_eq!(
            picked("根据下面的会议纪要起草一份通知", false),
            Route::Picked(MATERIAL.into()),
            "「根据……纪要」比「起草」更具体"
        );
        assert_eq!(
            picked("请仿照去年的冬季防火通知写一份今年的", false),
            Route::Picked(IMITATE.into()),
            "「仿照……」比「写一份」更具体"
        );
        assert_eq!(
            picked("起草复函，答复市林业局来函", false),
            Route::Picked(REPLY_LETTER.into())
        );
        let mut report = ctx("起草一份人工智能辅助决策的调研报告", false);
        report.kind = TemplateKind::ResearchReport;
        assert_eq!(route(&skills, &report), Route::Picked(POLICY_REPORT.into()));
        let Route::Ambiguous(tied) = picked("看看这个", true) else {
            panic!("都没命中应当交模型");
        };
        assert!(
            tied.len() >= 2 && !tied.contains(&POLICY_REPORT.to_string()),
            "{tied:?}"
        );
    }

    #[test]
    fn context_filters_respect_kind_selection_and_switches() {
        let text = "---\nname: 研究报告专用\napplies_to: [研究报告]\nwhen: { selection: required }\nflow: [{ step: generate }]\n---\n";
        let mut skill = parse("only", text, "测试").unwrap();
        let mut context = ctx("", true);
        assert!(!eligible(&skill, &context), "文种不对");
        context.kind = TemplateKind::ResearchReport;
        assert!(!eligible(&skill, &context), "没有选区");
        context.has_selection = true;
        assert!(eligible(&skill, &context));
        skill.enabled = false;
        assert!(!eligible(&skill, &context), "停用的不参与");
    }

    #[test]
    fn regex_triggers_score_by_the_matched_text() {
        let skill = parse(
            "x",
            "---\nname: 仿写\ntriggers: [仿照, 按照.*的格式, \"[坏\"]\n---\n",
            "测试",
        )
        .unwrap();
        assert_eq!(score(&skill, "请仿照去年的通知"), 2);
        assert_eq!(score(&skill, "按照去年那篇的格式写"), 9);
        assert_eq!(score(&skill, "随便写写"), 0);
    }

    #[test]
    fn a_slash_prefix_pins_the_skill() {
        let skills = builtin_skills();
        assert_eq!(
            slash(&skills, "/润色 压缩一下"),
            Some((POLISH.into(), "压缩一下"))
        );
        assert_eq!(
            slash(&skills, "／research-draft"),
            Some((RESEARCH_DRAFT.into(), ""))
        );
        assert_eq!(slash(&skills, "/没有这个 写"), None);
        assert_eq!(slash(&skills, "润色"), None);
    }

    struct Picker(&'static str);

    impl ModelBackend for Picker {
        fn complete(
            &self,
            _role: ModelRole,
            _system: &str,
            user: &str,
            _on_delta: &mut dyn FnMut(StreamDelta<'_>),
        ) -> anyhow::Result<Completion> {
            assert!(user.contains("1. 研究式起草") && user.contains("2. 润色"));
            Ok(Completion {
                content: self.0.into(),
                truncated: false,
            })
        }
        fn cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn the_model_chooses_from_a_closed_set() {
        let skills: Vec<Skill> = [RESEARCH_DRAFT, POLISH]
            .into_iter()
            .map(|id| crate::agent::skill::builtin(id).unwrap())
            .collect();
        let candidates: Vec<&Skill> = skills.iter().collect();
        assert_eq!(
            choose_by_model(&Picker("2"), &candidates, "看看").unwrap(),
            1
        );
        assert_eq!(
            choose_by_model(&Picker("选第 2 个"), &candidates, "看看").unwrap(),
            1
        );
        assert_eq!(
            choose_by_model(&Picker("9"), &candidates, "看看").unwrap(),
            0,
            "越界退回第一个"
        );
        assert_eq!(
            choose_by_model(&Picker("不知道"), &candidates, "看看").unwrap(),
            0
        );
    }
}
