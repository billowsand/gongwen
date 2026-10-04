//! `style_learn`：从 `@` 引用的几篇稿子学一份写法风格档案（`docs/ai-agent-workbench.md` 16.15 C.2）。
//!
//! 档案存进变量 `style_profile`（JSON），问题清单里列出名称与场合、写法描述和要删的样稿事实；
//! 存不存由用户在卡片上点「保存为风格」，流程本身不写 `styles.json`。

use super::{Flow, check_cancel, note, tool_line};
use crate::agent::board::Finding;
use crate::agent::engine::Event;
use crate::agent::skill::StepSpec;
use crate::agent::style::{self, MAX_SAMPLES, Sample};
use crate::agent::tools::{Permission, ToolCtx};

/// 学出的档案存在这个变量里（`StyleProfile` 的 JSON）。
pub(crate) const PROFILE_VAR: &str = "style_profile";

pub(super) fn style_learn(ctx: &mut ToolCtx<'_, '_>, _step: &StepSpec) -> anyhow::Result<Flow> {
    let refs = ctx.board.refs.clone();
    if refs.is_empty() {
        anyhow::bail!("先用 @ 引用要学的几篇稿子（2–10 篇同类的），再说「学一下这几篇的风格」。");
    }
    if refs.len() > MAX_SAMPLES {
        note(
            ctx,
            format!("一次最多学 {MAX_SAMPLES} 篇，只用前 {MAX_SAMPLES} 篇。"),
        );
    }
    let mut samples = Vec::new();
    for reference in refs.iter().take(MAX_SAMPLES) {
        match crate::agent::references::load_sample(ctx, reference) {
            Ok(sample) if !sample.text.trim().is_empty() => samples.push(sample),
            Ok(_) => note(ctx, format!("《{}》没有正文，跳过。", reference.title)),
            Err(error) => note(
                ctx,
                format!("《{}》读不出来（{error}），跳过。", reference.title),
            ),
        }
    }
    if samples.is_empty() {
        anyhow::bail!("引用的稿子都读不出正文，没法学。");
    }
    let chars: usize = samples.iter().map(|s| s.text.chars().count()).sum();
    tool_line(
        ctx,
        "style.learn",
        Permission::Read,
        format!("读样稿 {} 篇，共约 {chars} 字", samples.len()),
    );
    check_cancel(ctx)?;
    let learned = {
        let emit = &mut *ctx.emit;
        let samples: &[Sample] = &samples;
        style::learn(ctx.env.model, samples, ctx.env.vocabulary, &mut |phase| {
            emit(Event::Phase(phase))
        })?
    };
    let profile = &learned.profile;
    tool_line(
        ctx,
        "style.learn",
        Permission::Read,
        format!(
            "学出风格「{}」：范例 {} 段，{} 处样稿事实要删",
            profile.name,
            profile.examples.len(),
            learned.flagged.len()
        ),
    );
    let mut findings = vec![Finding {
        group: "风格档案".into(),
        text: format!(
            "{}（场合：{}）",
            profile.name,
            if profile.occasions.is_empty() {
                "未写".to_string()
            } else {
                profile.occasions.join("、")
            }
        ),
        excerpt: String::new(),
        source: format!("学自 {} 篇", profile.sources.len()),
        fix: None,
    }];
    findings.extend(
        profile
            .description
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| Finding {
                group: "写法描述".into(),
                text: line.trim().to_string(),
                excerpt: String::new(),
                source: String::new(),
                fix: None,
            }),
    );
    findings.extend(learned.flagged.iter().map(|value| Finding {
        group: "要删的样稿事实".into(),
        text: format!(
            "写法描述里出现了样稿里的「{value}」，只学写法不学事实，保存前到 AI 管理页删掉"
        ),
        excerpt: String::new(),
        source: String::new(),
        fix: None,
    }));
    ctx.board.findings.extend(findings);
    ctx.board
        .vars
        .insert(PROFILE_VAR.into(), serde_json::to_value(profile)?);
    Ok(Flow::Next)
}
