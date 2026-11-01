//! 侧栏里的「智能写作（Pi 试用）」入口。
//!
//! 它和普通技能发送是两条独立操作：用当前输入的要求、锁定选区、`@` 引用与粘贴
//! 材料开一轮 Pi Durable 文件工作区写作，结果仍然只能交成提案或侧栏清单，由用户
//! 接受。模型与生成参数只认「智能写作（Pi）」的独立配置，不回落起草模型。

use super::mention;
use crate::ai_panel::paste;
use crate::app::{DocJob, WorkerResult};
use crate::draft_page::DraftPage;
use crate::models::{LmStudioConfig, ReasoningEffort};
use crate::pi_agent::{
    PiBridge, PiEvent, PiJob, PiMaterialInput, PiModel, PiRuntime, PiSelection, PiTask,
    PiToolContext, PiWorkspace, base_dir, pi_skills, run,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// 一轮 Pi 写作的最长执行时间（等用户回答的时间不计）。到期由 Rust 与 host 两边止损。
const PI_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug)]
pub(crate) struct PiProposalSources {
    pub(crate) before: String,
    pub(crate) candidate: String,
    pub(crate) sources: Vec<String>,
}

impl PiProposalSources {
    pub(crate) fn for_document(&self, doc: &crate::draft_page::DraftSession) -> Vec<String> {
        if self.before == doc.generated_markdown
            && doc.ai_proposal.as_ref().is_some_and(|proposal| {
                proposal.before == self.before && proposal.result.markdown == self.candidate
            })
        {
            self.sources.clone()
        } else {
            Vec::new()
        }
    }
}

impl DraftPage<'_> {
    /// 开始一轮 Pi 试写。`editor_has_selection` 是点按钮时编辑器里是否有选区：
    /// 只支持侧栏里锁定过的选区；编辑器里临时选中而没有锁定就拒绝，不悄悄改全篇。
    pub(crate) fn start_pi_job(&mut self, editor_has_selection: bool) -> Result<(), String> {
        if self.doc.read_only() {
            return Err("这篇稿件已发布或归档，只读。".into());
        }
        if self.doc.busy || self.doc.ai_panel.running() {
            return Err("这篇稿件还有任务在跑，稍等一下。".into());
        }
        let composer = &self.doc.ai_panel.composer;
        if editor_has_selection && composer.selection.is_none() {
            return Err(
                "编辑器里选着文字，但侧栏没有锁定选区：请点输入框下方的「选区」锁定，\
                 或取消选中后按全文修改。"
                    .into(),
            );
        }
        let request = mention::plain_request(composer.text.trim());
        if request.trim().is_empty() {
            return Err("请先写下这轮要做什么，再点「智能写作（Pi 试用）」。".into());
        }
        let skill = pi_skills::resolve(composer.pi_skill.as_deref())?.map(str::to_string);
        // 输入框上的旧技能（15 个内置技能）不套用到 Pi：明确告诉模型与本轮记录，
        // 不静默忽略用户已经做出的选择。
        let old_skill = composer.skill.clone();
        let chat = self.config.pi_chat().map_err(|error| error.to_string())?;
        let model = pi_model(&chat)?;
        let kind = self.doc.draft.kind;
        let time = crate::prompt::TimeContext::now();
        let before = self.doc.generated_markdown.clone();
        let base = crate::app::ai_proposal::pi_base(self.doc);
        let previous_sources = self
            .doc
            .ai_panel
            .pi_sources
            .as_ref()
            .map(|sources| sources.for_document(self.doc))
            .unwrap_or_default();
        let selection = match &composer.selection {
            Some((range, text)) => {
                if before.get(range.clone()) != Some(text.as_str()) {
                    return Err("锁定选区之后正文又改过，请重新锁定选区再发送。".into());
                }
                Some(
                    PiSelection {
                        start: range.start,
                        end: range.end,
                    }
                    .validate(&before)?,
                )
            }
            None => None,
        };
        // 材料角色由用户选：事实材料 / 写法样稿 / 结构范文 / 来函。样稿不会进事实来源。
        let material_role = composer.pi_material_role;
        let ref_role = composer.pi_ref_role;
        let materials: Vec<PiMaterialInput> = composer
            .materials
            .iter()
            .map(|pasted: &paste::PastedFile| PiMaterialInput {
                title: pasted.material.name.clone(),
                source: "粘贴材料".into(),
                role: material_role,
                text: pasted.material.text.clone(),
            })
            .collect();
        let refs = mention::live_refs(&composer.text, &composer.refs);
        let mut prompt = request.clone();
        if let Some(skill) = &skill {
            prompt.push_str(&format!(
                "\n\n【使用技能】\n{skill}（skills/pi-skills/{skill}/SKILL.md）"
            ));
        }
        if let Some(old) = &old_skill {
            prompt.push_str(&format!(
                "\n\n【说明】输入框上选的旧技能「{old}」属于旧流程，不用于本轮 Pi；按上面的 Pi 技能或自动判断。"
            ));
        }
        if !composer.pi_notes.is_empty() {
            prompt.push_str("\n\n【此前已确认】\n");
            for note in &composer.pi_notes {
                prompt.push_str("- ");
                prompt.push_str(note);
                prompt.push('\n');
            }
        }
        if let Some(selection) = selection {
            prompt.push_str(&format!(
                "\n\n【只改选区】\n只允许改下面这段；选区以外的正文必须逐字保持原样，否则程序会拒绝提案。\n<<<选区\n{}\n选区>>>",
                selection.text(&before)
            ));
        }
        let title = match &skill {
            Some(skill) => format!("智能写作（Pi · {}）", pi_skills::label(skill)),
            None => "智能写作（Pi · 自动）".to_string(),
        };
        let mut context = vec![
            "Pi 文件工作区".to_string(),
            if before.trim().is_empty() {
                "空白起草".to_string()
            } else if base != before {
                "基于未采纳候选稿".to_string()
            } else {
                "修改现稿".to_string()
            },
            format!("材料 {} 份", materials.len() + refs.len()),
        ];
        if let Some(old) = &old_skill {
            context.push(format!("旧技能「{old}」未套用于 Pi"));
        }
        self.doc.ai_panel.open = true;
        self.doc.ai_proposal = None;
        self.doc.ai_review_baseline = Some(before.clone());
        self.doc.ai_prompt_last_label = title.clone();
        self.doc.ai_panel.pi_ask = None;
        self.doc.ai_panel.pi_report = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let bridge = Arc::new(PiBridge::default());
        self.doc.ai_panel.cancel = Some(cancel.clone());
        self.doc.ai_panel.pi_bridge = Some(bridge.clone());
        let turn_id = self
            .doc
            .ai_panel
            .push_turn(title.clone(), request.clone(), context, None);
        if let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) {
            turn.provenance = "Pi Durable · 文件工作区".into();
        }
        let (key, seq) = self.begin_job();
        if let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) {
            turn.usage_job_seq = Some(seq);
        }
        *self.status = "正在启动 Pi 智能写作…".into();

        let sender = self.sender.clone();
        let system_date = time.today.clone();
        let kind_label = kind.label().to_string();
        let config = self.config.clone();
        let draft_input = self.doc.draft.clone();
        let current = before.clone();
        let base_for_job = base.clone();
        let mut material_facts: Vec<String> = materials
            .iter()
            .filter(|material| material.role.is_fact_source())
            .map(|material| material.text.clone())
            .collect();
        material_facts.extend(previous_sources.clone());
        let probe = Arc::new(PiToolContext::new(
            Arc::new(config.clone()),
            draft_input.clone(),
            current.clone(),
            prompt.clone(),
            material_facts.clone(),
        ));
        std::thread::spawn(move || {
            let send = |job: DocJob| {
                let _ = sender.send(WorkerResult::Doc { key, seq, job });
            };
            let runtime = match PiRuntime::locate() {
                Ok(runtime) => runtime,
                Err(error) => {
                    send(DocJob::PiDone(Err(error)));
                    return;
                }
            };
            let base = match base_dir() {
                Ok(base) => base,
                Err(error) => {
                    send(DocJob::PiDone(Err(error)));
                    return;
                }
            };
            // `@` 引用在后台读成材料快照；任何一份读不到就明确失败，不静默丢。
            let mut materials = materials;
            if !previous_sources.is_empty() {
                materials.push(PiMaterialInput {
                    title: "上一轮已登记的原始依据".into(),
                    source: "仍有效的未采纳 Pi 提案（原始依据，不含候选稿）".into(),
                    role: crate::pi_agent::MaterialRole::Fact,
                    text: previous_sources.join("\n\n---\n\n"),
                });
            }
            for reference in &refs {
                match probe.read_reference(reference.source, reference.id) {
                    Ok((title, text)) => materials.push(PiMaterialInput {
                        title,
                        source: reference.source.label().to_string(),
                        role: ref_role,
                        text,
                    }),
                    Err(error) => {
                        send(DocJob::PiDone(Err(format!(
                            "读不到引用《{}》：{error}",
                            reference.title
                        ))));
                        return;
                    }
                }
            }
            let all_facts: Vec<String> = materials
                .iter()
                .filter(|material| material.role.is_fact_source())
                .map(|material| material.text.clone())
                .collect();
            let (workspace, metas) =
                match PiWorkspace::prepare(&base, key, seq, &current, &prompt, &materials) {
                    Ok(workspace) => workspace,
                    Err(error) => {
                        send(DocJob::PiDone(Err(error)));
                        return;
                    }
                };
            let workspace_label = workspace.label.clone();
            if base_for_job != current
                && let Err(error) = workspace.write_candidate(&base_for_job)
            {
                send(DocJob::PiDone(Err(error)));
                return;
            }
            let tools = Arc::new(PiToolContext::new(
                Arc::new(config),
                draft_input,
                current.clone(),
                prompt.clone(),
                all_facts,
            ));
            let job = PiJob {
                runtime,
                workspace,
                model,
                task: PiTask {
                    request: prompt.clone(),
                    kind: kind_label,
                    system_date,
                    skill,
                    selection,
                    materials: metas,
                },
                before: current,
                base: base_for_job,
                tools,
                bridge,
                request_id: format!("gongwen-{key}-{seq}"),
                run_id: uuid::Uuid::new_v4().to_string(),
                timeout: PI_TIMEOUT,
            };
            let result = run(job, cancel, |event| match event {
                PiEvent::Progress(message) => send(DocJob::PiProgress(message)),
                PiEvent::Tool(line) => send(DocJob::PiTool(line)),
                PiEvent::Evidence(line) => send(DocJob::PiTool(line)),
                PiEvent::Candidate { path, content } => send(DocJob::PiCandidate { path, content }),
                PiEvent::Questions(batch) => send(DocJob::PiQuestions(batch)),
            })
            .map_err(|error| {
                if error.contains(&workspace_label) {
                    error
                } else {
                    format!("{error}（工作区：{workspace_label}）")
                }
            });
            send(DocJob::PiDone(result));
        });
        Ok(())
    }
}

/// 应用现有 OpenAI 兼容模型设置映射成 host 的显式参数。
fn pi_model(chat: &LmStudioConfig) -> Result<PiModel, String> {
    let base_url = chat.base_url.trim().trim_end_matches('/').to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(format!(
            "智能写作模型的地址不是 http/https：{}。请到「AI 管理 → 模型服务」检查。",
            chat.base_url
        ));
    }
    let model_id = chat.model.trim().to_string();
    if model_id.is_empty() {
        return Err("还没有选择智能写作模型：请到「AI 管理 → 模型服务」选择。".into());
    }
    let context_window = if chat.context_window > 0 {
        chat.context_window
    } else {
        32_768
    };
    // 配置冲突时限制输出，不能虚构比服务端实际值更大的上下文窗口。
    let max_tokens = chat
        .max_tokens
        .max(1)
        .min(context_window.saturating_sub(1_024).max(1));
    let (reasoning, thinking_level) = match chat.reasoning_effort {
        ReasoningEffort::Default => (false, None),
        ReasoningEffort::Low => (true, Some("low".to_string())),
        ReasoningEffort::Medium => (true, Some("medium".to_string())),
        ReasoningEffort::High => (true, Some("high".to_string())),
        ReasoningEffort::Max => (true, Some("high".to_string())),
    };
    Ok(PiModel {
        base_url,
        api_key: chat.api_key.clone(),
        model_id,
        context_window,
        max_tokens,
        reasoning,
        thinking_level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_agent::MaterialRole;

    fn chat() -> LmStudioConfig {
        LmStudioConfig {
            base_url: "http://127.0.0.1:1234/v1/".into(),
            model: "local".into(),
            context_window: 8192,
            max_tokens: 32_000,
            reasoning_effort: ReasoningEffort::High,
            ..Default::default()
        }
    }

    #[test]
    fn model_limits_are_explicit_and_consistent() {
        let model = pi_model(&chat()).unwrap();
        assert_eq!(model.base_url, "http://127.0.0.1:1234/v1");
        assert_eq!(model.context_window, 8192);
        assert_eq!(model.max_tokens, 7168);
        assert!(model.max_tokens <= model.context_window);
        assert!(model.reasoning);
        assert_eq!(model.thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn non_http_and_empty_models_are_rejected() {
        let mut config = chat();
        config.base_url = "file:///c:/model".into();
        assert!(pi_model(&config).is_err());
        let mut config = chat();
        config.model.clear();
        assert!(pi_model(&config).is_err());
    }

    #[test]
    fn pi_model_is_independent_from_the_draft_model() {
        let mut config = crate::models::AppConfig::default();
        assert!(config.pi_chat().is_err(), "没选 Pi 模型时必须明确报错");
        config.providers.push(crate::models::ProviderConfig {
            id: "p1".into(),
            name: "本机".into(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            enabled: true,
            ..Default::default()
        });
        config.draft_model = crate::models::ModelRef {
            provider_id: "p1".into(),
            model: "draft-9b".into(),
        };
        assert!(config.pi_chat().is_err(), "不能偷偷回落到起草模型");
        config.pi_model = crate::models::ModelRef {
            provider_id: "p1".into(),
            model: "pi-large".into(),
        };
        let chat = config.pi_chat().unwrap();
        assert_eq!(chat.model, "pi-large");
        assert_eq!(config.draft_chat().unwrap().model, "draft-9b");
    }

    #[test]
    fn material_roles_travel_with_the_round() {
        let roles = [MaterialRole::Fact, MaterialRole::IncomingLetter];
        assert!(roles.iter().all(|role| role.is_fact_source()));
        assert!(!MaterialRole::StyleSample.is_fact_source());
        assert!(!MaterialRole::StructureSample.is_fact_source());
        assert_eq!(MaterialRole::default(), MaterialRole::Fact);
    }
}
