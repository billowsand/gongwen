//! 侧栏里的「智能写作（Pi 试用）」入口。
//!
//! 它和普通技能发送是两条独立操作：用当前输入的要求、锁定选区、`@` 引用与粘贴
//! 材料开一轮 Pi Durable 文件工作区写作，结果仍然只能交成提案或侧栏清单，由用户
//! 接受。模型与生成参数只认「智能写作（Pi）」的独立配置，不回落起草模型。
//!
//! 界面线程只做纯内存校验、创建等待中的轮次与任务；运行时定位、`@` 引用读取、
//! 工作区 / 材料快照、恢复快照与来源读取都在后台线程完成。准备好以后后台把稳定的
//! [`PiRunRef`] 交回界面，界面核对轮次并强制保存会话，确认之后才允许 host 启动。
//!
//! 中断、停止、失败以后可以「接着写」：重新打开同一个工作区与 Durable 库，用同一份
//! requestId 找回原来的提交；也可以「从头写」：按原要求与材料、**当前**文种与日期开
//! 一份新工作区。两条路都先校验工作区路径、快照哈希、host 版本与当前正文基线。

use super::mention;
use super::pi_prepare::PiPrepareAck;
use crate::ai_panel::paste;
use crate::ai_services::references::Reference;
use crate::app::{DocJob, WorkerResult};
use crate::draft_page::DraftPage;
use crate::models::{LmStudioConfig, ReasoningEffort};
use crate::pi_agent::{
    MaterialRole, PiBridge, PiEvent, PiJob, PiMaterialInput, PiModel, PiOutcome, PiRunLog,
    PiRunRecord, PiRunRef, PiRunSelection, PiRuntime, PiSelection, PiStrategyPreference, PiTask,
    PiToolContext, PiWorkspace, base_dir, pi_skills, read_sources, resolve_workspace, run,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// 一轮 Pi 写作的最长执行时间（等用户回答的时间不计）。到期由 Rust 与 host 两边止损。
const PI_TIMEOUT: Duration = Duration::from_secs(900);
/// 准备完成后等界面确认运行引用的上限；超过就不启动 host，避免挂住后台线程。
const PREPARE_ACK_TIMEOUT: Duration = Duration::from_secs(30);

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

/// 后台准备完成后，界面核对轮次并保存运行引用，再通过它确认给后台。
/// 后台准备阶段的全部输入。界面只搬内存里的东西，不在这里读文件 / 跑子进程。
struct PiPrepare {
    key: u64,
    seq: u64,
    turn_id: u64,
    restart: bool,
    reference: Option<PiRunRef>,
    config: Arc<crate::models::AppConfig>,
    draft: crate::models::DraftInput,
    before: String,
    base: String,
    prompt: String,
    raw_request: Option<String>,
    kind: String,
    system_date: String,
    skill: Option<String>,
    strategy: PiStrategyPreference,
    selection: Option<PiSelection>,
    materials: Vec<PiMaterialInput>,
    refs: Vec<Reference>,
    ref_roles: Vec<MaterialRole>,
    previous_sources: Vec<String>,
    model: PiModel,
    model_provider: String,
    model_id: String,
}

/// 准备完成、可以启动 host 的一份任务。
struct PiLaunch {
    runtime: PiRuntime,
    workspace: PiWorkspace,
    model: PiModel,
    task: PiTask,
    before: String,
    base: String,
    tools: Arc<PiToolContext>,
    bridge: Arc<PiBridge>,
    record: Arc<PiRunLog>,
    request_id: String,
    run_id: String,
    resume: bool,
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
            return Err("请先写下这轮要做什么，再发送给 Pi。".into());
        }
        let selected = composer.pi_skill.clone();
        self.doc.ai_panel.ensure_pi_skills();
        let catalog = self
            .doc
            .ai_panel
            .pi_skills
            .as_ref()
            .expect("刚刷新过 Pi 技能目录");
        let skill = catalog.resolve(selected.as_deref())?;
        let skill_label = skill.as_deref().map(|name| catalog.label_for(name));
        let composer = &self.doc.ai_panel.composer;
        let chat = self.config.pi_chat().map_err(|error| error.to_string())?;
        let model = pi_model(&chat)?;
        let model_provider = self.config.pi_model.provider_id.clone();
        let model_id = self.config.pi_model.model.clone();
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
        // 材料角色逐篇取自用户选择：事实材料 / 写法样稿 / 结构范文 / 来函。样稿不会进事实来源。
        let strategy = composer.pi_strategy;
        let materials: Vec<PiMaterialInput> = composer
            .materials
            .iter()
            .enumerate()
            .map(
                |(index, pasted): (usize, &paste::PastedFile)| PiMaterialInput {
                    title: pasted.material.name.clone(),
                    source: "粘贴材料".into(),
                    role: composer.material_role(index),
                    text: pasted.material.text.clone(),
                },
            )
            .collect();
        let refs = mention::live_refs(&composer.text, &composer.refs);
        // 逐篇引用用途按「来源 + 编号」算，同名不同来源不串用。
        let ref_roles: Vec<MaterialRole> = refs
            .iter()
            .map(|reference| composer.ref_role(reference.source, reference.id))
            .collect();
        let mut prompt = request.clone();
        if let Some(skill) = &skill {
            prompt.push_str(&format!(
                "\n\n【使用技能】\n{skill}（skills/pi-skills/{skill}/SKILL.md）"
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
            Some(_) => format!(
                "智能写作（Pi · {}）",
                skill_label.as_deref().unwrap_or("技能")
            ),
            None => "智能写作（Pi · 自动）".to_string(),
        };
        let context = vec![
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

        // 界面线程到此为止：只创建等待中的轮次，准备交给后台。
        let (key, seq) = self.begin_job();
        let turn_id = self.begin_pi_turn(&title, &request, &context, seq);
        self.open_pi_panel(&title, seq);
        *self.status = "正在准备 Pi 工作区…".into();
        let prepare = PiPrepare {
            key,
            seq,
            turn_id,
            restart: false,
            reference: None,
            config: Arc::new(self.config.clone()),
            draft: self.doc.draft.clone(),
            before,
            base,
            prompt,
            raw_request: Some(request),
            kind: kind.label().to_string(),
            system_date: time.today,
            skill,
            strategy,
            selection,
            materials,
            refs,
            ref_roles,
            previous_sources,
            model,
            model_provider,
            model_id,
        };
        spawn_prepare(prepare, self.panel_cancel(), self.sender.clone());
        Ok(())
    }

    /// 把一份 Pi 轮次原样搬回输入区：材料逐篇用途取自那一轮工作区的材料索引（模型实际
    /// 读到的那份快照），策略倾向取自运行记录，要求用卡片上显示的原话。改完再发就是新的
    /// 一轮，原工作区与记录都不动。
    fn copy_pi_run_into_composer(
        &mut self,
        reference: &PiRunRef,
        prompt: &str,
    ) -> Result<(), String> {
        if reference.is_empty() || reference.run_id.trim().is_empty() {
            return Err("这一轮没有可读的 Pi 工作区，复制不出当时的材料。".into());
        }
        let root = resolve_workspace(&reference.workspace)?;
        let workspace = PiWorkspace::reopen(root, reference.workspace.clone())?;
        let record = PiRunRecord::read_from(&workspace.run)?;
        if record.run_id != reference.run_id
            || record.request_id != reference.request_id
            || record.workspace != reference.workspace
        {
            return Err("运行记录与这一轮的引用不一致，不能复制。".into());
        }
        record.check_request()?;
        record.check_materials(&workspace)?;
        let inputs = workspace.read_material_texts()?;
        let mut pasted = Vec::new();
        let mut roles = Vec::new();
        for material in inputs {
            pasted.push(
                super::paste::PastedFile::new(&material.text)
                    .map_err(|error| format!("临时材料创建失败（{error}）"))?,
            );
            // 当时逐篇选的用途原样跟着走，不降成事实材料。
            roles.push(material.role);
        }
        let count = pasted.len();
        let composer = &mut self.doc.ai_panel.composer;
        composer.text = record.user_request.clone().unwrap_or_else(|| {
            // 旧选区轮次没有单独保存原始要求时不猜边界，避免把旧选区当成新要求。
            if record.selection.is_some() {
                String::new()
            } else {
                prompt.to_string()
            }
        });
        composer.pi_skill = record.skill.clone();
        // 那一轮的 `@` 引用在发送时已经变成带用途的材料，这里不再当成引用搬一遍。
        composer.refs.clear();
        composer.materials = pasted;
        composer.pi_material_roles = roles;
        composer.pi_ref_roles.clear();
        composer.pi_notes = record.confirmed.clone();
        // 策略倾向按运行记录，不看输入框现在的选择。
        composer.pi_strategy = record.strategy;
        composer.selection = None;
        composer.error = None;
        self.doc.ai_panel.open = true;
        *self.status = format!(
            "已把这一轮 Pi 的要求、{count} 份材料（按当时用途）与策略复制到输入区；检查后点发送就是新的一轮，原运行记录保留。"
        );
        Ok(())
    }

    /// 从旧「分阶段写作」面板显式迁移到统一 Pi 入口：搬主题、素材（按原用途）、
    /// 归纳出的写法与结构，按分阶段倾向开新任务。旧面板随后收起，旧流程不再运行。
    pub(crate) fn copy_writing_task_to_pi(&mut self) -> bool {
        if self.doc.read_only() {
            *self.status = "这篇稿件已发布或归档，只读。".into();
            return false;
        }
        if self.doc.busy || self.doc.ai_panel.running() || self.doc.writing.running {
            *self.status = "这篇稿件还有任务在跑，稍等一下再迁移旧任务。".into();
            return false;
        }
        let (text, pasted, roles) = match self.writing_task_materials(String::new()) {
            Ok(parts) => parts,
            Err(error) => {
                *self.status = error;
                return false;
            }
        };
        if text.trim().is_empty() && pasted.is_empty() {
            *self.status = "旧分阶段写作还没有主题和素材，没什么可迁移的。".into();
            return false;
        }
        let count = pasted.len();
        let composer = &mut self.doc.ai_panel.composer;
        composer.text = text;
        // 旧面板没有 `@` 引用，已有的引用也不能跟着串进来。
        composer.refs.clear();
        composer.materials = pasted;
        composer.pi_material_roles = roles;
        composer.pi_ref_roles.clear();
        composer.pi_notes.clear();
        composer.pi_strategy = PiStrategyPreference::Phased;
        composer.pi_skill = Some("gongwen-draft".into());
        composer.selection = None;
        composer.error = None;
        self.doc.ai_panel.open = true;
        self.doc.writing.open = false;
        *self.status = format!(
            "已把旧分阶段写作的主题与 {count} 份素材（按原用途）复制到右侧智能写作；检查后点发送，旧任务历史仍保留。"
        );
        true
    }

    /// 旧「分阶段写作」任务的素材与归纳结果，按原来的内容 / 风格 / 结构用途打包。
    fn writing_task_materials(
        &self,
        fallback_text: String,
    ) -> Result<(String, Vec<super::paste::PastedFile>, Vec<MaterialRole>), String> {
        let task = &self.doc.writing.task;
        let text = if task.topic.trim().is_empty() {
            fallback_text
        } else {
            task.topic.clone()
        };
        let mut pasted = Vec::new();
        let mut roles = Vec::new();
        for source in &task.sources {
            pasted.push(
                super::paste::PastedFile::new(&source.text)
                    .map_err(|error| format!("临时材料创建失败（{error}）"))?,
            );
            // 内容 / 风格 / 结构三种素材按原用途搬：不编造、也不降成普通事实材料。
            roles.push(match source.role {
                crate::agent::writing::SourceRole::Content => MaterialRole::Fact,
                crate::agent::writing::SourceRole::Style => MaterialRole::StyleSample,
                crate::agent::writing::SourceRole::Framework => MaterialRole::StructureSample,
            });
        }
        for (title, body, role) in [
            (
                "旧任务归纳的写法",
                task.style_md.trim(),
                MaterialRole::StyleSample,
            ),
            (
                "旧任务归纳的结构",
                task.framework_md.trim(),
                MaterialRole::StructureSample,
            ),
        ] {
            if body.is_empty() {
                continue;
            }
            pasted.push(
                super::paste::PastedFile::new(&format!("# {title}\n\n{body}"))
                    .map_err(|error| format!("临时材料创建失败（{error}）"))?,
            );
            roles.push(role);
        }
        Ok((text, pasted, roles))
    }

    /// 把旧流程轮次显式复制成一份新的 Pi 任务：只搬要求、材料、引用与已确认信息，
    /// 旧选区不迁到当前正文（旧检查点也不能当成 Pi 检查点），要写什么由用户再看一遍。
    ///
    /// 两条旧历史都能搬：
    /// - 旧侧栏流程轮次（有 `request`）：材料与引用按事实材料，不继承输入框里残留的用途；
    /// - 旧「分阶段写作」任务（无 `request`，成果挂在这一轮）：素材按原来的内容 / 风格 /
    ///   结构用途搬过来，归纳出来的写法与结构也跟着搬，不丢用户历史任务，也不重新跑旧流程。
    pub(crate) fn copy_turn_to_pi(&mut self, turn_id: u64) -> Result<(), String> {
        if self.doc.read_only() {
            return Err("这篇稿件已发布或归档，只读。".into());
        }
        if self.doc.busy || self.doc.ai_panel.running() {
            return Err("这篇稿件还有任务在跑，稍等一下再复制旧任务。".into());
        }
        let (pi_run, prompt, request, staged) = {
            let Some(turn) = self
                .doc
                .ai_panel
                .turns
                .iter()
                .find(|turn| turn.id == turn_id)
            else {
                return Err("这一轮已经不在了。".into());
            };
            // 旧分阶段写作的成果也挂在这一轮里，要求不在 `request` 上，在写作任务里。
            // 刚跑完时由 `proposal_turn` 指认；重开会话后按主题与卡片抬头对上。
            let topic = self.doc.writing.task.topic.trim();
            let staged = turn.request.is_none()
                && (self.doc.writing.proposal_turn == Some(turn_id)
                    || (!topic.is_empty() && topic == turn.prompt.trim()));
            (
                turn.pi_run.clone(),
                turn.prompt.clone(),
                turn.request.clone(),
                staged,
            )
        };
        if let Some(reference) = pi_run {
            // Pi 轮次：按那一轮工作区里的材料用途与运行记录里的策略原样搬回输入区。
            return self.copy_pi_run_into_composer(&reference, &prompt);
        }
        let mut pasted = Vec::new();
        let mut roles = Vec::new();
        let text;
        let mut refs = Vec::new();
        let mut notes = Vec::new();
        let mut migrated_staged = false;
        if let Some(request) = request {
            text = request.text.clone();
            refs = request.refs.clone();
            notes = request.notes.clone();
            for material in &request.materials {
                pasted.push(
                    super::paste::PastedFile::new(&material.text)
                        .map_err(|error| format!("临时材料创建失败（{error}）"))?,
                );
                // 旧会话没有逐篇用途：一律按事实材料，不继承输入框里残留的角色。
                roles.push(MaterialRole::Fact);
            }
        } else if staged {
            let (task_text, task_pasted, task_roles) = self.writing_task_materials(prompt)?;
            text = task_text;
            pasted = task_pasted;
            roles = task_roles;
            migrated_staged = true;
        } else {
            return Err("这一轮没有可复制的原始要求。".into());
        }
        if text.trim().is_empty() && pasted.is_empty() {
            return Err("这一轮没有可复制的主题与材料，不用迁移。".into());
        }
        let count = pasted.len();
        let composer = &mut self.doc.ai_panel.composer;
        composer.text = text;
        composer.refs = refs;
        composer.pi_skill = None;
        composer.materials = pasted;
        composer.pi_material_roles = roles;
        // 引用用途按「来源 + 编号」存：旧任务没记过用途，不能留着输入框里上一篇的角色。
        composer.pi_ref_roles.clear();
        composer.pi_notes = notes;
        // 旧分阶段写作本来就是一步一步跑的，搬过来继续按分阶段；旧侧栏流程不动用户的选择。
        if migrated_staged {
            composer.pi_strategy = PiStrategyPreference::Phased;
        }
        // 旧选区、旧提案与旧检查点都不迁移：新正文基线可能已经变了。
        composer.selection = None;
        composer.error = None;
        self.doc.ai_panel.open = true;
        *self.status = if migrated_staged {
            format!(
                "已把旧分阶段写作的主题、{count} 份素材（按原用途）与归纳结果复制到统一 Pi 入口；检查后点发送，旧任务历史仍保留。"
            )
        } else {
            format!(
                "已把旧任务的要求、{count} 份材料与引用复制到统一 Pi 入口；检查后点发送，旧任务历史仍保留。"
            )
        };
        Ok(())
    }

    /// 接着写（`restart == false`）或从头写（`restart == true`）。
    pub(crate) fn resume_pi_job(&mut self, turn_id: u64, restart: bool) -> Result<(), String> {
        if self.doc.read_only() {
            return Err("这篇稿件已发布或归档，只读，不能恢复 Pi 写作。".into());
        }
        if self.doc.busy || self.doc.ai_panel.running() {
            return Err("这篇稿件还有任务在跑，稍等一下。".into());
        }
        let (turn_title, reference) = {
            let Some(turn) = self.doc.ai_panel.turns.iter().find(|t| t.id == turn_id) else {
                return Err("这一轮已经不在了。".into());
            };
            if !matches!(
                turn.state,
                crate::ai_panel::TurnState::Interrupted
                    | crate::ai_panel::TurnState::Stopped
                    | crate::ai_panel::TurnState::Failed(_)
            ) {
                return Err("这一轮现在不是中断 / 停止 / 失败状态，不能恢复。".into());
            }
            let Some(reference) = turn.pi_run.clone().filter(PiRunRef::recoverable) else {
                return Err("这一轮没有留下可恢复的 Pi 运行记录。".into());
            };
            (turn.title.clone(), reference)
        };
        let chat = self.config.pi_chat().map_err(|error| error.to_string())?;
        let model = pi_model(&chat)?;
        let kind = self.doc.draft.kind;
        let time = crate::prompt::TimeContext::now();
        let before = self.doc.generated_markdown.clone();
        let base = crate::app::ai_proposal::pi_base(self.doc);
        let context = if restart {
            vec!["Pi 文件工作区".to_string(), "从头重写".to_string()]
        } else {
            vec!["Pi 文件工作区".to_string(), "接着上次的进度".to_string()]
        };

        let (key, seq) = self.begin_job();
        if let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) {
            turn.state = crate::ai_panel::TurnState::Waiting;
            turn.phase = if restart {
                "正在重开工作区…".into()
            } else {
                "正在从上次中断处接着写…".into()
            };
            turn.usage_job_seq = Some(seq);
            turn.started = Instant::now();
            turn.elapsed = None;
            turn.context = context.clone();
        }
        self.open_pi_panel(&turn_title, seq);
        *self.status = if restart {
            "正在准备 Pi 工作区…".into()
        } else {
            "正在从上次中断处恢复 Pi 智能写作…".into()
        };
        let prepare = PiPrepare {
            key,
            seq,
            turn_id,
            restart,
            reference: Some(reference),
            config: Arc::new(self.config.clone()),
            draft: self.doc.draft.clone(),
            before,
            base,
            prompt: String::new(),
            raw_request: None,
            kind: kind.label().to_string(),
            system_date: time.today,
            skill: None,
            // 恢复时不用这里的值：策略倾向按运行记录（`PiRunRecord.strategy`）走，
            // 免得重启后输入框里的选择把常规任务改成阶段性。
            strategy: self.doc.ai_panel.composer.pi_strategy,
            selection: None,
            materials: Vec::new(),
            refs: Vec::new(),
            ref_roles: Vec::new(),
            previous_sources: Vec::new(),
            model,
            model_provider: self.config.pi_model.provider_id.clone(),
            model_id: self.config.pi_model.model.clone(),
        };
        spawn_prepare(prepare, self.panel_cancel(), self.sender.clone());
        Ok(())
    }

    /// 在侧栏登记一个等待中的 Pi 轮次（界面线程，纯内存）。
    fn begin_pi_turn(&mut self, title: &str, prompt: &str, context: &[String], seq: u64) -> u64 {
        let turn_id = self.doc.ai_panel.push_turn(
            title.to_string(),
            prompt.to_string(),
            context.to_vec(),
            None,
        );
        if let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) {
            turn.provenance = "Pi Durable · 文件工作区".into();
            turn.usage_job_seq = Some(seq);
            turn.phase = "正在准备 Pi 工作区…".into();
        }
        turn_id
    }

    /// 打开侧栏、挂上本轮取消标志与提问通道；不动已有提案（准备失败要能继续审阅）。
    fn open_pi_panel(&mut self, title: &str, _seq: u64) {
        self.doc.ai_panel.open = true;
        self.doc.ai_panel.pi_ask = None;
        self.doc.ai_panel.pi_report = None;
        self.doc.ai_prompt_last_label = title.to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        self.doc.ai_panel.cancel = Some(cancel);
        self.doc.ai_panel.pi_bridge = None;
    }

    fn panel_cancel(&self) -> Arc<AtomicBool> {
        self.doc
            .ai_panel
            .cancel
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
    }

    /// 会话重载后继续改未采纳提案：把这一轮落盘的原始依据读回来。
    pub(crate) fn restore_pi_sources(&mut self, turn_id: u64) {
        let Some(reference) = self
            .doc
            .ai_panel
            .turns
            .iter()
            .find(|turn| turn.id == turn_id)
            .and_then(|turn| turn.pi_run.clone())
        else {
            return;
        };
        let Some(expected) = reference.sources.clone() else {
            return;
        };
        let Some(proposal) = self.doc.ai_proposal.as_ref() else {
            return;
        };
        if proposal.before != self.doc.generated_markdown {
            return;
        }
        let Ok(root) = resolve_workspace(&reference.workspace) else {
            return;
        };
        let Some(run_dir) = root.parent() else {
            return;
        };
        let Ok(Some(text)) = read_sources(run_dir, &expected) else {
            return;
        };
        let sources: Vec<String> = text.split("\n\n---\n\n").map(str::to_string).collect();
        let Some(turn) = self
            .doc
            .ai_panel
            .turns
            .iter()
            .find(|turn| turn.id == turn_id)
        else {
            return;
        };
        let request = turn.prompt.clone();
        self.doc.ai_panel.pi_sources = Some(PiProposalSources {
            before: proposal.before.clone(),
            candidate: proposal.result.markdown.clone(),
            sources: sources
                .into_iter()
                .chain(std::iter::once(request))
                .collect(),
        });
    }
}

/// 后台准备 + 确认 + 启动 host。所有失败都走 `PiDone(Err)`，不会把 `busy` 留在 true。
fn spawn_prepare(prepare: PiPrepare, cancel: Arc<AtomicBool>, sender: Sender<WorkerResult>) {
    spawn_prepare_with(prepare, cancel, sender, prepare_pi);
}

fn spawn_prepare_with(
    prepare: PiPrepare,
    cancel: Arc<AtomicBool>,
    sender: Sender<WorkerResult>,
    work: impl FnOnce(PiPrepare, &AtomicBool) -> Result<PiLaunch, String> + Send + 'static,
) {
    let key = prepare.key;
    let seq = prepare.seq;
    let turn_id = prepare.turn_id;
    std::thread::spawn(move || {
        let send = |job: DocJob| {
            let _ = sender.send(WorkerResult::Doc { key, seq, job });
        };
        let launch = match work(prepare, &cancel) {
            Ok(launch) => launch,
            Err(error) => {
                send(DocJob::PiDone(Box::new(Err(error))));
                return;
            }
        };
        // 准备完成：先把稳定运行引用交回界面，等它核对轮次并保存会话。
        let reference = PiRunRef::from_record(&launch.record.snapshot());
        let dev_runtime = !launch.runtime.bundled;
        let (ack, rx) = PiPrepareAck::channel();
        send(DocJob::PiPrepared {
            turn_id,
            reference,
            dev_runtime,
            bridge: launch.bridge.clone(),
            kind: launch.task.kind.clone(),
            ack,
        });
        if let Err(error) = wait_prepared(&rx, &cancel) {
            send(DocJob::PiDone(Box::new(Err(error))));
            return;
        }
        let workspace_label = launch.workspace.label.clone();
        let result = run_launch(launch, cancel, |event| match event {
            PiEvent::Progress(message) => send(DocJob::PiProgress(message)),
            PiEvent::Strategy(info) => {
                let line = info.line();
                send(DocJob::PiTool(line.clone()));
                send(DocJob::PiProgress(line));
            }
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
        send(DocJob::PiDone(Box::new(result)));
    });
}

/// 有界等待界面确认；取消或界面丢弃事件都会让后台立即结束。
fn wait_prepared(rx: &Receiver<Result<(), String>>, cancel: &AtomicBool) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已停止 Pi 智能写作，未启动 host。".into());
        }
        if started.elapsed() > PREPARE_ACK_TIMEOUT {
            return Err("界面没有确认运行引用，未启动 Pi host。".into());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("这一轮已经被停止或关闭，未启动 Pi host。".into());
            }
        }
    }
}

/// 后台准备：运行时定位、引用读取、工作区 / 材料快照、恢复读取都在这里。
fn prepare_pi(prepare: PiPrepare, cancel: &AtomicBool) -> Result<PiLaunch, String> {
    prepare_pi_with_runtime(prepare, cancel, PiRuntime::locate)
}

fn prepare_pi_with_runtime(
    mut prepare: PiPrepare,
    cancel: &AtomicBool,
    locate: impl FnOnce() -> Result<PiRuntime, String>,
) -> Result<PiLaunch, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止 Pi 智能写作，未启动 host。".into());
    }
    // 窗口探测放在后台准备线程里：LM Studio 的实际加载窗口只有原生
    // `/api/v0/models` 给，标准 `/v1/models` 只有 ID。短超时、只问已配置的地址，
    // 不阻塞界面，也不盲探无关主机。
    refine_model_window(&mut prepare);
    let runtime = locate()?;
    if prepare.reference.is_some() {
        prepare_resume(prepare, runtime, cancel)
    } else {
        prepare_start(prepare, runtime, cancel)
    }
}

/// 在后台把 `peek_window` 的估计换成服务实际报告的加载窗口。
fn refine_model_window(prepare: &mut PiPrepare) {
    let Ok(chat) = prepare.config.pi_chat() else {
        return;
    };
    let mut probe = chat;
    probe.timeout_seconds = probe.timeout_seconds.clamp(1, 3);
    let window = crate::lmstudio::context::window(&probe);
    let tokens = window.tokens.min(u32::MAX as usize).max(1) as u32;
    prepare.model.context_window = tokens;
    prepare.model.max_tokens = prepare
        .model
        .max_tokens
        .min(tokens.saturating_sub(1_024).max(1));
}

fn prepare_start(
    prepare: PiPrepare,
    runtime: PiRuntime,
    cancel: &AtomicBool,
) -> Result<PiLaunch, String> {
    let PiPrepare {
        key,
        seq,
        config,
        draft,
        before,
        base,
        prompt,
        raw_request,
        kind,
        system_date,
        skill,
        strategy,
        selection,
        mut materials,
        refs,
        ref_roles,
        previous_sources,
        model,
        model_provider,
        model_id,
        ..
    } = prepare;
    let mut probe_facts: Vec<String> = materials
        .iter()
        .filter(|material| material.role.is_fact_source())
        .map(|material| material.text.clone())
        .collect();
    probe_facts.extend(previous_sources.clone());
    let probe = PiToolContext::new(
        config.clone(),
        draft.clone(),
        before.clone(),
        prompt.clone(),
        probe_facts,
    );
    if !previous_sources.is_empty() {
        materials.push(PiMaterialInput {
            title: "上一轮已登记的原始依据".into(),
            source: "仍有效的未采纳 Pi 提案（原始依据，不含候选稿）".into(),
            role: MaterialRole::Fact,
            text: previous_sources.join("\n\n---\n\n"),
        });
    }
    for (index, reference) in refs.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("已停止 Pi 智能写作，未启动 host。".into());
        }
        let (title, text) = probe
            .read_reference(reference.source, reference.id)
            .map_err(|error| format!("读不到引用《{}》：{error}", reference.title))?;
        materials.push(PiMaterialInput {
            title,
            source: reference.source.label().to_string(),
            role: ref_roles.get(index).copied().unwrap_or(MaterialRole::Fact),
            text,
        });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止 Pi 智能写作，未启动 host。".into());
    }
    let workspaces = base_dir()?;
    let catalog = pi_skills::load_catalog();
    let (workspace, metas) = PiWorkspace::prepare(
        &workspaces,
        key,
        &before,
        &prompt,
        &materials,
        &catalog,
        skill.as_deref(),
    )?;
    // 先写候选稿（会往 request.md 追加说明），再取实际输入文件的哈希；顺序反了
    // 记录里的 input_request 就对应旧内容，恢复必然被拒。
    let input_request = finalize_input(&workspace, &base, &before)?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let request_id = format!("gongwen-{key}-{seq}");
    let mut record = PiRunRecord::start(
        run_id.clone(),
        request_id.clone(),
        &workspace,
        &before,
        &base,
        &before,
        &input_request,
        (base != before).then_some(base.as_str()),
        &prompt,
        &kind,
        &system_date,
        skill.as_deref(),
        selection.map(|selection| PiRunSelection {
            start: selection.start,
            end: selection.end,
            sha256: crate::pi_agent::sha256_hex(selection.text(&before)),
        }),
        metas.clone(),
        &model_provider,
        &model_id,
    )
    .with_strategy(strategy);
    record.user_request = raw_request;
    let log = Arc::new(PiRunLog::open(&workspace, record)?);
    let tools = Arc::new(PiToolContext::new(
        config,
        draft,
        before.clone(),
        prompt.clone(),
        materials
            .iter()
            .filter(|material| material.role.is_fact_source())
            .map(|material| material.text.clone())
            .collect(),
    ));
    Ok(PiLaunch {
        runtime,
        workspace,
        model,
        task: PiTask {
            request: prompt,
            kind,
            system_date,
            skill,
            strategy,
            selection,
            materials: metas,
        },
        before,
        base,
        tools,
        bridge: Arc::new(PiBridge::default()),
        record: log,
        request_id,
        run_id,
        resume: false,
    })
}

fn prepare_resume(
    prepare: PiPrepare,
    runtime: PiRuntime,
    _cancel: &AtomicBool,
) -> Result<PiLaunch, String> {
    let PiPrepare {
        key,
        seq,
        reference,
        config,
        draft,
        before,
        base,
        kind,
        system_date,
        restart,
        strategy: _,
        model,
        model_provider,
        model_id,
        ..
    } = prepare;
    let reference = reference.ok_or("缺少 Pi 运行引用，不能恢复")?;
    let old_root = resolve_workspace(&reference.workspace)?;
    let old_workspace = PiWorkspace::reopen(old_root, reference.workspace.clone())?;
    let record = PiRunRecord::read_from(&old_workspace.run)?;
    if record.run_id != reference.run_id
        || record.request_id != reference.request_id
        || record.workspace != reference.workspace
    {
        return Err("运行记录与当前轮次引用不一致，不能恢复或重写。".into());
    }
    record.check_request()?;
    // 策略倾向用**运行记录里的**：输入框现在选的是什么与这一轮无关，恢复中途换策略会把
    // 常规任务变成分阶段（或反之）。旧记录没有这一项，读回来就是自动。
    let strategy = record.strategy;
    // 旧材料只做路径 / 哈希 / 体积校验：两种恢复都要求材料没被换过。
    record.check_materials(&old_workspace)?;
    // 文种换了就不能沿用旧规则写、再用新文种清洗。

    if restart {
        // 从头写：不校验旧正文 / 旧模型 / 旧 Durable 库，只沿用旧要求与材料，
        // 以**当前**正文、**当前**文种 / 日期与当前 Pi 模型建立新运行记录。
        let inputs = old_workspace
            .read_material_texts()
            .map_err(|error| format!("读不回上一轮的材料，不能从头重写：{error}"))?;
        let selection = restart_selection(record.selection.as_ref(), &before)?;
        let restored = read_sources(&old_workspace.run, record.sources.as_deref().unwrap_or(""))
            .ok()
            .flatten();
        let workspaces = base_dir()?;
        let catalog = pi_skills::load_catalog();
        let (workspace, metas) = PiWorkspace::prepare(
            &workspaces,
            key,
            &before,
            &record.request,
            &inputs,
            &catalog,
            record.skill.as_deref(),
        )?;
        let input_request = finalize_input(&workspace, &base, &before)?;
        let material_facts: Vec<String> = inputs
            .iter()
            .filter(|material| material.role.is_fact_source())
            .map(|material| material.text.clone())
            .chain(restored.iter().flat_map(|text| split_sources(text)))
            .collect();
        let run_id = uuid::Uuid::new_v4().to_string();
        let request_id = format!("gongwen-{key}-{seq}");
        let mut new_record = PiRunRecord::start(
            run_id.clone(),
            request_id.clone(),
            &workspace,
            &before,
            &base,
            &before,
            &input_request,
            (base != before).then_some(base.as_str()),
            &record.request,
            &kind,
            &system_date,
            record.skill.as_deref(),
            selection.map(|selection| PiRunSelection {
                start: selection.start,
                end: selection.end,
                sha256: crate::pi_agent::sha256_hex(selection.text(&before)),
            }),
            metas.clone(),
            &model_provider,
            &model_id,
        )
        .with_strategy(record.strategy);
        new_record.user_request = record.user_request.clone();
        let log = Arc::new(PiRunLog::open(&workspace, new_record)?);
        let tools = Arc::new(PiToolContext::new(
            config,
            draft,
            before.clone(),
            record.request.clone(),
            material_facts,
        ));
        // 上一轮用户确认过的回答仍是事实（纯答案值，不含模型题面假设）。
        tools.confirm_answers(&record.answers);
        return Ok(PiLaunch {
            runtime,
            workspace,
            model,
            task: PiTask {
                request: record.request,
                kind,
                system_date,
                skill: record.skill,
                strategy,
                selection,
                materials: metas,
            },
            before,
            base,
            tools,
            bridge: Arc::new(PiBridge::default()),
            record: log,
            request_id,
            run_id,
            resume: false,
        });
    }

    // 接着写：完整校验旧正文基线、host 版本、Durable 库、选区与模型。
    check_resume_kind(&record.kind, &kind)?;
    record.check_resume(&reference.workspace, &reference.run_id, &before)?;
    record.check_selection(&before)?;
    crate::pi_agent::recovery::check_workspace_files(&old_workspace, &record)?;
    if model_provider != record.model_provider || model_id != record.model {
        return Err(format!(
            "这一轮用的是 {} / {}，现在配置的是 {} / {}。请改回原模型再「接着写」，或点「从头写」用当前模型重开一轮。",
            record.model_provider, record.model, model_provider, model_id
        ));
    }
    let selection = record.selection.as_ref().map(|selection| PiSelection {
        start: selection.start,
        end: selection.end,
    });
    let selection = match selection {
        Some(selection) => Some(selection.validate(&before)?),
        None => None,
    };
    // 上次工具检索与回答产生的真实事实来源必须重灌；丢了会让恢复后的交付闸门
    // 把真实事实当成无来源。文件哈希不符就明确拒绝，不静默降级。
    let restored = match record.sources.as_deref() {
        Some(hash) => match read_sources(&old_workspace.run, hash)? {
            Some(text) => Some(text),
            None => {
                return Err("本轮事实来源文件与运行记录对不上，不能接着写；请「从头写」。".into());
            }
        },
        None => None,
    };
    let candidate_path = old_workspace.root.join("input/candidate.md");
    let candidate_text = if record.candidate.is_some() {
        String::from_utf8(crate::pi_agent::recovery::read_bounded(
            &candidate_path,
            crate::pi_agent::recovery::MAX_SNAPSHOT_BYTES,
        )?)
        .map_err(|_| "工作区里的候选稿不是 UTF-8".to_string())?
    } else {
        String::new()
    };
    let inputs = old_workspace.read_material_texts()?;
    let material_facts: Vec<String> = inputs
        .iter()
        .filter(|material| material.role.is_fact_source())
        .map(|material| material.text.clone())
        .chain(restored.iter().flat_map(|text| split_sources(text)))
        .collect();
    let materials = record.materials.clone();
    let tools = Arc::new(PiToolContext::new(
        config,
        draft,
        before.clone(),
        record.request.clone(),
        material_facts,
    ));
    // 已确认回答只重灌纯答案值；「问题：回答」的显示字符串可能含模型题面假设。
    tools.confirm_answers(&record.answers);
    let log = Arc::new(PiRunLog::open_existing(&old_workspace, record.clone())?);
    Ok(PiLaunch {
        runtime,
        workspace: old_workspace,
        model,
        task: PiTask {
            request: record.request,
            kind: record.kind,
            system_date: record.system_date,
            skill: record.skill,
            strategy,
            selection,
            materials,
        },
        before,
        base: if candidate_text.is_empty() {
            base
        } else {
            candidate_text
        },
        tools,
        bridge: Arc::new(PiBridge::default()),
        record: log,
        request_id: reference.request_id,
        run_id: reference.run_id,
        resume: true,
    })
}

/// 启动 host 进程。只在界面确认运行引用之后调用。
fn run_launch(
    launch: PiLaunch,
    cancel: Arc<AtomicBool>,
    on_event: impl FnMut(PiEvent),
) -> Result<PiOutcome, String> {
    let PiLaunch {
        runtime,
        workspace,
        model,
        task,
        before,
        base,
        tools,
        bridge,
        record,
        request_id,
        run_id,
        resume,
    } = launch;
    let job = PiJob {
        runtime,
        workspace,
        model,
        task,
        before,
        base,
        tools,
        bridge,
        record,
        resume,
        request_id,
        run_id,
        timeout: PI_TIMEOUT,
    };
    run(job, cancel, on_event)
}

/// 写候选稿（会往 `request.md` 追加说明），再读取实际输入文件算哈希。
fn finalize_input(workspace: &PiWorkspace, base: &str, before: &str) -> Result<String, String> {
    if base != before {
        workspace.write_candidate(base)?;
    }
    workspace.read_request()
}

/// 接着写时必须沿用原文种；换了文种就拒绝，避免按旧规则生成、再用新文种清洗。
fn check_resume_kind(record_kind: &str, current_kind: &str) -> Result<(), String> {
    if record_kind == current_kind {
        Ok(())
    } else {
        Err(format!(
            "这一轮是按「{record_kind}」跑的，当前文种是「{current_kind}」。请切回文种后「接着写」，或用「从头写」按当前文种重开一轮。"
        ))
    }
}

/// 从头写时对旧选区的处理：位置与原文哈希都对得上才沿用；对不上直接拒绝，
/// 不把旧偏移套到新正文上，也不默默放宽成全文（那会违背“只改选区”）。
fn restart_selection(
    saved: Option<&PiRunSelection>,
    before: &str,
) -> Result<Option<PiSelection>, String> {
    let Some(saved) = saved else {
        return Ok(None);
    };
    let stale = || {
        "上次锁定的选区在当前正文里已经对不上（正文改过或选区被删）。请重新锁定选区后再发送；\
         「从头写」不会放宽到全文。"
            .to_string()
    };
    let Some(text) = before.get(saved.start..saved.end) else {
        return Err(stale());
    };
    if crate::pi_agent::sha256_hex(text) != saved.sha256 {
        return Err(stale());
    }
    Ok(Some(PiSelection {
        start: saved.start,
        end: saved.end,
    }))
}

/// `pi/sources.md` 里用 `---` 分隔的多份来源。
fn split_sources(text: &str) -> Vec<String> {
    text.split("\n\n---\n\n")
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

impl PiRunLog {
    /// 恢复时重新接上原记录：**只**用 `from_record` 接上内存记录，不写回 `run.json`。
    ///
    /// 运行状态由主进程在取得工作区所有权（host 已拿到 Node 锁）之后才更新；
    /// 这里先写回 Running 会在第二个窗口还没拿到锁时覆盖第一个窗口的记录。
    pub(crate) fn open_existing(
        workspace: &PiWorkspace,
        record: PiRunRecord,
    ) -> Result<Self, String> {
        Self::from_record(workspace.clone(), record)
    }
}

/// 应用现有 OpenAI 兼容模型设置映射成 host 的显式参数。
///
/// 思考深度取自 `AppConfig::pi_chat`（Pi 自己的 `pi_options`，不沿用文字起草）；
/// 窗口与输出预算仍由程序自动解析。
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
    let context_window = crate::lmstudio::context::peek_window(chat)
        .tokens
        .min(u32::MAX as usize) as u32;
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

    fn preparation() -> PiPrepare {
        PiPrepare {
            key: 1,
            seq: 1,
            turn_id: 1,
            restart: false,
            reference: None,
            config: Arc::new(crate::models::AppConfig::default()),
            draft: crate::models::DraftInput::default(),
            before: "当前正文".into(),
            base: "当前正文".into(),
            prompt: "用户要求".into(),
            raw_request: Some("用户要求".into()),
            kind: "研究报告".into(),
            system_date: "2026-11-02".into(),
            skill: None,
            strategy: PiStrategyPreference::Auto,
            selection: None,
            materials: vec![],
            refs: vec![],
            ref_roles: vec![],
            previous_sources: vec![],
            model: pi_model(&chat()).unwrap(),
            model_provider: "当前服务商".into(),
            model_id: "当前模型".into(),
        }
    }

    #[test]
    fn slow_preparation_returns_immediately_and_failure_is_reported() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let before = Instant::now();
        spawn_prepare_with(preparation(), cancel.clone(), sender, move |_, flag| {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
            assert!(flag.load(Ordering::Relaxed));
            Err("准备已取消".into())
        });
        assert!(before.elapsed() < Duration::from_secs(1));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        release_tx.send(()).unwrap();
        let WorkerResult::Doc {
            job: DocJob::PiDone(result),
            ..
        } = receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("准备失败必须统一收尾")
        };
        assert!(result.is_err());
    }

    #[test]
    fn restarting_uses_the_original_request_but_the_current_kind_and_date() {
        struct ConfigReset;
        impl Drop for ConfigReset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let _reset = ConfigReset;
        let (workspace, metas) = PiWorkspace::prepare(
            &base_dir().unwrap(),
            1,
            "旧正文",
            "保留的原要求",
            &[],
            &pi_skills::load_catalog(),
            None,
        )
        .unwrap();
        let record = PiRunRecord::start(
            "old-run".into(),
            "old-request".into(),
            &workspace,
            "旧正文",
            "旧正文",
            "旧正文",
            &workspace.read_request().unwrap(),
            None,
            "保留的原要求",
            "通知",
            "2026-10-01",
            None,
            None,
            metas,
            "原服务商",
            "原模型",
        )
        .with_strategy(PiStrategyPreference::Phased);
        record.write(&workspace).unwrap();
        let mut prepare = preparation();
        prepare.reference = Some(PiRunRef::from_record(&record));
        prepare.restart = true;
        prepare.prompt.clear();
        // 重启后输入框可能已经回到「自动」：恢复/重开必须按运行记录里的「倾向分阶段」走。
        assert_eq!(prepare.strategy, PiStrategyPreference::Auto);
        let launch = prepare_pi_with_runtime(prepare, &AtomicBool::new(false), || {
            Ok(PiRuntime {
                node: dir.path().join("unused-node"),
                host: dir.path().join("unused-host"),
                bundled: false,
            })
        })
        .unwrap();
        assert!(!launch.resume);
        assert_ne!(launch.run_id, "old-run");
        assert_eq!(launch.task.request, "保留的原要求");
        assert_eq!(launch.task.kind, "研究报告");
        assert_eq!(launch.task.system_date, "2026-11-02");
        assert_eq!(launch.before, "当前正文");
        assert_eq!(
            launch.task.strategy,
            PiStrategyPreference::Phased,
            "重开也用运行记录里的策略倾向，不看输入框当前选择"
        );
        let saved = launch.record.snapshot();
        assert_eq!(saved.model, "当前模型");
        assert_eq!(saved.baseline, crate::pi_agent::sha256_hex("当前正文"));
        assert_eq!(saved.strategy, PiStrategyPreference::Phased);
    }

    #[test]
    fn resuming_keeps_the_recorded_strategy_instead_of_the_composer() {
        struct ConfigReset;
        impl Drop for ConfigReset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let _reset = ConfigReset;
        let (workspace, metas) = PiWorkspace::prepare(
            &base_dir().unwrap(),
            1,
            "当前正文",
            "保留的原要求",
            &[],
            &pi_skills::load_catalog(),
            None,
        )
        .unwrap();
        // 恢复会校验 Durable 库还在。
        std::fs::write(&workspace.state, b"sqlite").unwrap();
        let record = PiRunRecord::start(
            "old-run".into(),
            "old-request".into(),
            &workspace,
            "当前正文",
            "当前正文",
            "当前正文",
            &workspace.read_request().unwrap(),
            None,
            "保留的原要求",
            "研究报告",
            "2026-11-02",
            None,
            None,
            metas,
            "当前服务商",
            "当前模型",
        )
        .with_strategy(PiStrategyPreference::Phased);
        record.write(&workspace).unwrap();
        let mut prepare = preparation();
        prepare.reference = Some(PiRunRef::from_record(&record));
        // 重启后输入框回到了「自动」：恢复不能因此把阶段任务变成常规任务。
        prepare.strategy = PiStrategyPreference::Auto;
        let launch = prepare_pi_with_runtime(prepare, &AtomicBool::new(false), || {
            Ok(PiRuntime {
                node: dir.path().join("unused-node"),
                host: dir.path().join("unused-host"),
                bundled: false,
            })
        })
        .unwrap();
        assert!(launch.resume);
        assert_eq!(launch.task.strategy, PiStrategyPreference::Phased);
        assert_eq!(
            launch.record.snapshot().strategy,
            PiStrategyPreference::Phased,
            "恢复不会把记录里的倾向改写"
        );
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
        config.lm_studio.max_tokens = 1;
        config.lm_studio.context_window = 8_192;
        config.pi_model.model = "deepseek-flash".into();
        let pi = config.pi_chat().unwrap();
        assert_eq!(pi.max_tokens, 32_768, "不继承旧起草输出参数");
        assert_eq!(pi.context_window, 0, "Pi 独立自动解析窗口");
        let model = pi_model(&pi).unwrap();
        assert_eq!(
            model.context_window, 1_048_576,
            "复用已维护的服务/模型窗口解析器"
        );
        assert_eq!(model.max_tokens, 32_768);
        assert_eq!(config.draft_chat().unwrap().context_window, 8_192);
    }

    #[test]
    fn a_stale_selection_is_rejected_on_restart_instead_of_widening_to_full_text() {
        let saved = PiRunSelection {
            start: 2,
            end: 8,
            sha256: crate::pi_agent::sha256_hex("正文"),
        };
        let selection = restart_selection(Some(&saved), "ab正文cd").unwrap();
        assert_eq!(selection.map(|s| (s.start, s.end)), Some((2, 8)));
        // 同一偏移的原文已经换了：直接拒绝，不默默改成全文。
        let error = restart_selection(Some(&saved), "ab别的cd").unwrap_err();
        assert!(error.contains("选区"), "{error}");
        assert!(error.contains("不会放宽"), "{error}");
        // 越界同样拒绝。
        assert!(restart_selection(Some(&saved), "短").is_err());
        // 没有旧选区时照常返回 None。
        assert!(restart_selection(None, "任意正文").unwrap().is_none());
    }

    #[test]
    fn resuming_with_a_different_document_kind_is_rejected() {
        check_resume_kind("通知", "通知").unwrap();
        let error = check_resume_kind("通知", "请示").unwrap_err();
        assert!(error.contains("通知"), "{error}");
        assert!(error.contains("请示"), "{error}");
        assert!(error.contains("从头写"), "{error}");
    }

    #[test]
    fn candidate_note_is_written_before_the_request_hash_is_taken() {
        let config = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(config.path().to_path_buf()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        let dir = tempfile::tempdir().unwrap();
        let (workspace, _) = PiWorkspace::prepare(
            dir.path(),
            1,
            "正式正文",
            "用户要求：改短",
            &[],
            &pi_skills::load_catalog(),
            None,
        )
        .unwrap();
        // 旧顺序（先 read_request 再 write_candidate）会记下旧内容；新顺序必须对上实际文件。
        let before_candidate = workspace.read_request().unwrap();
        let hash = finalize_input(&workspace, "未采纳的候选稿", "正式正文").unwrap();
        let actual = workspace.read_request().unwrap();
        assert_ne!(before_candidate, actual);
        assert_eq!(hash, actual);
        assert!(actual.contains("继续修改未采纳稿"));
        // 没有候选稿时不应改动要求文件。
        let unchanged = finalize_input(&workspace, "正式正文", "正式正文").unwrap();
        assert_eq!(unchanged, actual);
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
