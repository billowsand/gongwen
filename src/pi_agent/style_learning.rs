//! 设置页的风格学习适配：只组装样稿、启动现有Pi、验证风格文件，不另写模型循环。
use super::{
    MaterialRole, PiBridge, PiEvent, PiJob, PiMaterialInput, PiOutcome, PiRunLog, PiRunRecord,
    PiRuntime, PiTask, PiToolContext, PiWorkspace, pi_skills,
};
use crate::ai_services::style::{self, Learned, Sample, StyleProfile, StyleSource};
use crate::models::{AppConfig, DraftInput, TemplateKind};
use serde::Deserialize;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StyleDraft {
    name: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    occasions: Vec<String>,
    description: String,
    #[serde(default)]
    example_numbers: Vec<usize>,
}

pub(crate) fn validate_draft(content: &str) -> Result<StyleDraft, String> {
    if content.len() > 64 * 1024 {
        return Err("风格文件超过64 KiB。".into());
    }
    let draft: StyleDraft =
        serde_json::from_str(content).map_err(|e| format!("风格JSON格式不正确：{e}"))?;
    if draft.name.trim().is_empty()
        || draft.name.chars().count() > 40
        || draft.note.chars().count() > 500
        || draft.description.trim().is_empty()
        || draft.description.chars().count() > 12000
        || draft.occasions.len() > 8
        || draft
            .occasions
            .iter()
            .any(|s| s.trim().is_empty() || s.chars().count() > 60)
        || draft.example_numbers.len() > 5
        || draft.example_numbers.iter().any(|n| !(1..=30).contains(n))
        || draft
            .example_numbers
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != draft.example_numbers.len()
    {
        return Err("风格档案字段为空、超限或范例编号不合法。".into());
    }
    Ok(draft)
}

fn profile_from_draft(
    content: &str,
    samples: &[Sample],
    workspace: &str,
) -> Result<Learned, String> {
    let draft = validate_draft(content)?;
    let candidates = style::candidate_examples(samples);
    if draft.example_numbers.iter().any(|n| *n > candidates.len()) {
        return Err("风格范例编号不在本轮候选清单中。".into());
    }
    let examples = if draft.example_numbers.is_empty() {
        candidates.into_iter().take(3).collect()
    } else {
        draft
            .example_numbers
            .iter()
            .map(|n| candidates[n - 1].clone())
            .collect()
    };
    let mut kinds = Vec::new();
    for kind in samples.iter().filter_map(|sample| sample.kind) {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    Ok(Learned {
        profile: StyleProfile {
            name: draft.name,
            note: draft.note,
            occasions: draft.occasions,
            description: draft.description,
            stats: style::compute_stats(samples),
            examples,
            kinds,
            sources: samples
                .iter()
                .filter(|s| s.id > 0)
                .map(|s| StyleSource {
                    source: s.source,
                    id: s.id,
                    title: s.title.clone(),
                    hash: style::content_hash(&s.text),
                })
                .collect(),
            learned_at: chrono::Local::now().to_rfc3339(),
            learning_workspace: workspace.into(),
            ..Default::default()
        },
        flagged: Vec::new(),
    })
}

/// 上传样稿只保留在Pi工作区，不自动导入知识库。旧工作区和材料哈希都验证后才读。
pub(crate) fn uploaded_samples(label: &str) -> Result<Vec<Sample>, String> {
    if label.is_empty() {
        return Ok(Vec::new());
    }
    let workspace = PiWorkspace::reopen(super::resolve_workspace(label)?, label.into())?;
    let record = PiRunRecord::read_from(&workspace.run)?;
    record.check_materials(&workspace)?;
    if record.kind != "风格档案" || record.skill.as_deref() != Some(pi_skills::STYLE_SKILL) {
        return Err("该工作区不是风格学习的样稿快照。".into());
    }
    let metas = workspace.read_materials()?;
    let texts = workspace.read_material_texts()?;
    Ok(metas
        .into_iter()
        .zip(texts)
        .filter(|(meta, _)| meta.source == "上传样稿" && meta.role == MaterialRole::StyleSample)
        .map(|(_, material)| Sample {
            source: crate::ai_services::references::RefSource::Knowledge,
            id: 0,
            title: material.title,
            kind: None,
            text: material.text,
        })
        .collect())
}

pub(crate) fn learn(
    config: Arc<AppConfig>,
    samples: Vec<Sample>,
    cancel: Arc<AtomicBool>,
    mut on_phase: impl FnMut(String),
) -> Result<Learned, String> {
    if samples.is_empty()
        || samples.len() > style::MAX_SAMPLES
        || samples.iter().any(|s| s.text.trim().is_empty())
    {
        return Err("请选择1–10篇非空样稿。".into());
    }
    if samples.iter().any(|sample| sample.text.len() > 512 * 1024)
        || samples
            .iter()
            .map(|sample| sample.text.len())
            .sum::<usize>()
            > 4 * 1024 * 1024
    {
        return Err("样稿单篇超过512 KiB或合计超过4 MiB，请缩减材料。".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("风格学习已停止。".into());
    }
    let mut chat = config.pi_chat().map_err(|e| e.to_string())?;
    chat.timeout_seconds = chat.timeout_seconds.clamp(1, 3);
    crate::lmstudio::context::window(&chat);
    let model = crate::ai_panel::pi_job::pi_model(&chat)?;
    let runtime = PiRuntime::locate()?;
    let stats = style::compute_stats(&samples);
    let candidates = style::candidate_examples(&samples);
    let request = format!(
        "从本轮全部样稿学习一份可复用风格。只学写法，不学事项事实；最后交work/style.json，使用submit_style，不写公文。\n程序统计：{}\n候选范例（编号从1开始，JSON数组）：{}\nJSON字段为name、note、occasions、description、example_numbers，不填写其他字段。",
        stats.lines().join("；"),
        serde_json::to_string(&candidates).map_err(|e| e.to_string())?
    );
    let materials: Vec<_> = samples
        .iter()
        .map(|sample| PiMaterialInput {
            title: sample.title.clone(),
            source: if sample.id > 0 {
                sample.source.label().into()
            } else {
                "上传样稿".into()
            },
            role: MaterialRole::StyleSample,
            text: sample.text.clone(),
        })
        .collect();
    let catalog = pi_skills::load_catalog();
    let (workspace, metas) = PiWorkspace::prepare(
        &super::base_dir()?,
        0,
        "",
        &request,
        &materials,
        &catalog,
        Some(pi_skills::STYLE_SKILL),
    )?;
    let guide = serde_json::json!({"stats": stats, "candidates": candidates});
    let guide = serde_json::to_vec(&guide).map_err(|e| e.to_string())?;
    crate::vault::write_path(&workspace.root.join("input/style-guide.json"), "pi", &guide)
        .map_err(|e| format!("保存风格学习数据失败：{e:#}"))?;
    let label = workspace.label.clone();
    let run_id = uuid::Uuid::new_v4().to_string();
    let request_id = format!("style-{}", uuid::Uuid::new_v4());
    let today = crate::prompt::TimeContext::now().today;
    let record = PiRunRecord::start(
        run_id.clone(),
        request_id.clone(),
        &workspace,
        "",
        "",
        "",
        &workspace.read_request()?,
        None,
        &request,
        "风格档案",
        &today,
        Some(pi_skills::STYLE_SKILL),
        None,
        metas.clone(),
        &config.pi_model.provider_id,
        &model.model_id,
    );
    let record = Arc::new(PiRunLog::open(&workspace, record)?);
    // 风格学习只用选择的样稿，host不注册知识库/表单/正文工具，也不注入真实组织背景。
    let tools = Arc::new(PiToolContext::new(
        Arc::new(AppConfig::default()),
        DraftInput {
            kind: TemplateKind::PlainDocument,
            ..Default::default()
        },
        String::new(),
        String::new(),
        Vec::new(),
    ));
    on_phase("Pi 正在阅读样稿并学习风格…".into());
    let outcome = super::run(
        PiJob {
            runtime,
            workspace,
            model,
            task: PiTask {
                request,
                kind: "风格档案".into(),
                system_date: today,
                skill: Some(pi_skills::STYLE_SKILL.into()),
                strategy: Default::default(),
                capability: Default::default(),
                selection: None,
                materials: metas,
            },
            before: String::new(),
            base: String::new(),
            tools,
            bridge: Arc::new(PiBridge::default()),
            record,
            resume: false,
            request_id,
            run_id,
            timeout: Duration::from_secs(1800),
            proxy_env: crate::net::proxy_env(&config.proxy),
        },
        cancel.clone(),
        |event| {
            if let PiEvent::Progress(message) | PiEvent::Tool(message) = event {
                on_phase(message);
            }
        },
    )
    .map_err(|e| format!("{e}；工作文件保留在 {label}"))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("风格学习已停止，未采纳本轮结果。".into());
    }
    match outcome {
        PiOutcome::Style(delivery) => {
            profile_from_draft(&delivery.content, &samples, &delivery.workspace_label)
        }
        _ => Err("Pi 没有明确提交风格文件，未创建档案。".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_services::references::RefSource;

    fn samples() -> Vec<Sample> {
        vec![
            Sample {
                source: RefSource::Manuscript,
                id: 7,
                title: "库内样稿".into(),
                kind: Some(TemplateKind::PlainDocument),
                text: format!(
                    "# 通知\n\n{}",
                    "为推动工作落实，各单位要明确责任、协同推进，按既定安排完成任务。".repeat(3)
                ),
            },
            Sample {
                source: RefSource::Knowledge,
                id: 0,
                title: "上传样稿.md".into(),
                kind: None,
                text: "为做好专项工作，各单位要完善组织安排，推动工作落实，及时总结进展和经验。"
                    .repeat(3),
            },
        ]
    }

    #[test]
    fn artifact_shape_rejects_authority_fields_and_does_not_accept_prose() {
        for content in [
            r#"{"name":"风格","description":"说明","enabled":true}"#,
            r#"{"name":"风格","description":""}"#,
            r#"{"name":"风格","description":"说明","example_numbers":[1,1]}"#,
            "已完成，风格庄重简洁。",
        ] {
            assert!(validate_draft(content).is_err(), "{content}");
        }
    }

    #[test]
    fn profile_stats_examples_and_provenance_come_from_actual_selected_samples() {
        let samples = samples();
        let content = r#"{"name":"简洁部署","note":"任务导向","description":"先目的后安排，段落简洁。","occasions":["部署"],"example_numbers":[1]}"#;
        let learned =
            profile_from_draft(content, &samples, "pi/workspaces/doc-0/run-test").unwrap();
        assert_eq!(learned.profile.stats.docs, 2);
        assert_eq!(learned.profile.sources.len(), 1, "上传样稿不冒充知识库条目");
        assert_eq!(learned.profile.sources[0].id, 7);
        assert_eq!(
            learned.profile.examples[0],
            style::candidate_examples(&samples)[0]
        );
        assert_eq!(learned.profile.kinds, vec![TemplateKind::PlainDocument]);
        assert!(!learned.profile.learning_workspace.is_empty());
        assert!(learned.flagged.is_empty(), "不再产生事实核查清单");
        assert!(
            profile_from_draft(
                r#"{"name":"风格","description":"说明","example_numbers":[30]}"#,
                &samples,
                ""
            )
            .is_err()
        );
    }

    #[test]
    fn uploaded_samples_are_read_from_hash_verified_pi_snapshots() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let temp = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(temp.path().to_owned()));
        let _reset = Reset;
        let skills = pi_skills::SkillSnapshot::build_builtin(Some(pi_skills::STYLE_SKILL)).unwrap();
        let materials = vec![PiMaterialInput {
            title: "上传.md".into(),
            source: "上传样稿".into(),
            role: MaterialRole::StyleSample,
            text: "原始样稿".into(),
        }];
        let (workspace, metas) = PiWorkspace::prepare_with_snapshot(
            &super::super::base_dir().unwrap(),
            0,
            "",
            "学风格",
            &materials,
            &skills,
        )
        .unwrap();
        let record = PiRunRecord::start(
            "run".into(),
            "request".into(),
            &workspace,
            "",
            "",
            "",
            &workspace.read_request().unwrap(),
            None,
            "学风格",
            "风格档案",
            "2031-03-16",
            Some(pi_skills::STYLE_SKILL),
            None,
            metas,
            "test",
            "test",
        );
        record.write(&workspace).unwrap();
        let loaded = uploaded_samples(&workspace.label).unwrap();
        assert_eq!(loaded[0].text, "原始样稿");
        assert_eq!(loaded[0].id, 0);
        std::fs::write(workspace.root.join("input/materials/01.md"), "样稿被替换").unwrap();
        assert!(uploaded_samples(&workspace.label).is_err());
        assert!(uploaded_samples("../../outside").is_err());
    }
}
