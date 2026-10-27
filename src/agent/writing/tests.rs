//! 分阶段写作的流程与来源回归测试。

use super::*;
use crate::agent::testkit::ScriptedModel;

#[test]
fn every_content_chunk_is_visited_and_all_writing_stages_inject_style() {
    let evidence = "末尾独有证据。";
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "基层防汛工作".into();
    task.style_md = "STYLE-MARKER：短句，先结论后依据。".into();
    task.framework_md = "FRAMEWORK-MARKER：先说明问题，再列措施。".into();
    task.add_source(
        SourceSnapshot::new(
            "长材料",
            SourceRole::Content,
            format!("{}{}", "前段材料。".repeat(6000), evidence),
        )
        .unwrap(),
    );
    let model = ScriptedModel::new(move |_, prompt| {
        if prompt.contains("请拟定提纲") || prompt.contains("请调整提纲") {
            "章节：工作要求 | 说明任务与措施 | 500字".into()
        } else if prompt.contains("当前章节素材组") || prompt.contains("正文片段") {
            "现已落实有关措施。".into()
        } else if prompt.contains(evidence) {
            format!("观点：已落实防汛措施\n原句：{evidence}")
        } else {
            "无相关内容".into()
        }
    });
    let mut snapshots = 0;
    let mut persist = |_: &WritingTask| {
        snapshots += 1;
        Ok(())
    };
    run(&mut task, Action::Extract, &model, &mut persist).unwrap();
    let content_chunks = chunks(&task.sources[0].text, task.chunk_bytes).len();
    assert!(content_chunks > 20);
    assert_eq!(task.coverage().done, content_chunks);
    assert!(task.topic_cards.iter().any(|card| card.quote == evidence));
    assert_eq!(task.validate_sources().len(), 0);

    run(&mut task, Action::Plan, &model, &mut persist).unwrap();
    assert!(
        model
            .prompts("STYLE-MARKER")
            .iter()
            .any(|p| p.contains("请拟定提纲"))
    );
    task.confirm_outline().unwrap();
    run(&mut task, Action::Collect, &model, &mut persist).unwrap();
    assert!(
        model
            .prompts("原文材料")
            .iter()
            .all(|p| !p.contains("FRAMEWORK-MARKER"))
    );
    run(&mut task, Action::Write, &model, &mut persist).unwrap();
    assert!(
        model
            .prompts("STYLE-MARKER")
            .iter()
            .any(|p| p.contains("当前章节素材组"))
    );
    run(&mut task, Action::Polish, &model, &mut persist).unwrap();
    assert!(
        model
            .prompts("STYLE-MARKER")
            .iter()
            .any(|p| p.contains("正文片段"))
    );
    assert!(!task.final_markdown.is_empty());
    assert!(snapshots > content_chunks);
    assert_eq!(task.resume_action, None);
}

#[test]
fn input_invalidation_requires_outline_confirmation_again_and_preserves_human_text() {
    let mut task = WritingTask::new(crate::models::TemplateKind::PlainDocument);
    task.sections.push(Section {
        title: "人工章节".into(),
        markdown: "人工已经修改的正文".into(),
        manually_edited: true,
        ..Default::default()
    });
    task.outline_confirmed = true;
    task.invalidate_from(Stage::Extract);
    assert!(!task.outline_confirmed);
    assert_eq!(task.sections[0].markdown, "人工已经修改的正文");
}

#[test]
fn changed_outline_discards_section_coverage_but_keeps_topic_coverage() {
    let mut task = WritingTask::new(crate::models::TemplateKind::PlainDocument);
    let source = SourceSnapshot::new("材料", SourceRole::Content, "一项原文事实。").unwrap();
    task.add_source(source.clone());
    task.sections.push(Section {
        id: "s".into(),
        title: "第一节".into(),
        markdown: "人工改文".into(),
        manually_edited: true,
        chunk_bytes: task.chunk_bytes,
        ..Default::default()
    });
    let (start, end, _) = chunks(&source.text, task.chunk_bytes).remove(0);
    let hash = digest(format!("{}:{}:{}", source.hash, start, end).as_bytes());
    for section_id in [None, Some("s".into())] {
        task.chunks.push(ChunkProgress {
            source_id: source.id.clone(),
            chunk_hash: hash.clone(),
            role: SourceRole::Content,
            section_id,
            start,
            end,
            status: ChunkStatus::Extracted,
            error: None,
            output: String::new(),
        });
    }
    task.outline_confirmed = true;
    assert!(task.ready_for(Stage::Write).is_ok());
    task.invalidate_from(Stage::Collect);
    assert_eq!(task.coverage().done, 1);
    assert_eq!(task.section_coverage("s").done, 0);
    assert!(task.ready_for(Stage::Write).is_err());
    assert_eq!(task.sections[0].markdown, "人工改文");
}

#[test]
fn partial_quotes_are_not_accepted_as_a_successful_extraction() {
    let mut task = WritingTask::new(crate::models::TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.add_source(SourceSnapshot::new("材料", SourceRole::Content, "真实原句。").unwrap());
    let model = ScriptedModel::new(|_, _| {
        "观点：真实观点\n原句：真实原句。\n观点：伪造观点\n原句：不存在的原句。".into()
    });
    assert!(run(&mut task, Action::Extract, &model, &mut |_| Ok(())).is_err());
    assert_eq!(task.coverage().done, 0);
    assert_eq!(task.coverage().failed, 1);
    assert!(task.topic_cards.is_empty());
    assert_eq!(task.resume_action, Some(Action::Extract));
}

#[test]
fn failed_persistence_stops_before_any_model_request() {
    let mut task = WritingTask::new(crate::models::TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.add_source(SourceSnapshot::new("材料", SourceRole::Content, "原文。").unwrap());
    let model = ScriptedModel::new(|_, _| panic!("保存失败后不得调用模型"));
    assert!(
        run(&mut task, Action::Extract, &model, &mut |_| anyhow::bail!(
            "模拟磁盘失败"
        ))
        .is_err()
    );
    assert_eq!(task.coverage().done, 0);
}

#[test]
fn resume_skips_successful_blocks_and_resets_the_call_budget() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.add_source(
        SourceSnapshot::new("长材料", SourceRole::Content, "原文段落。".repeat(500)).unwrap(),
    );
    let first = ScriptedModel::new(|_, _| "无相关内容".into());
    assert!(
        run(&mut task, Action::Extract, &first, &mut |task| {
            if task.coverage().done == 1 {
                anyhow::bail!("模拟中断")
            }
            Ok(())
        })
        .is_err()
    );
    assert_eq!(task.coverage().done, 1);
    let remaining = task.coverage().pending;
    let second = ScriptedModel::new(|_, _| "无相关内容".into());
    run(&mut task, Action::Extract, &second, &mut |_| Ok(())).unwrap();
    assert_eq!(second.prompts("原文块").len(), remaining);
    assert_eq!(task.coverage().pending, 0);
    assert_eq!(task.resume_action, None);
}

#[test]
fn truncated_output_is_not_counted_as_a_successful_block() {
    struct Truncated;
    impl crate::agent::backend::ModelBackend for Truncated {
        fn complete(
            &self,
            _: crate::agent::backend::ModelRole,
            _: &str,
            _: &str,
            _: &mut dyn FnMut(crate::lmstudio::StreamDelta<'_>),
        ) -> anyhow::Result<crate::agent::backend::Completion> {
            Ok(crate::agent::backend::Completion {
                content: "观点：未完成\n原句：原文".into(),
                truncated: true,
            })
        }
        fn cancelled(&self) -> bool {
            false
        }
    }
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.add_source(SourceSnapshot::new("材料", SourceRole::Content, "原文。").unwrap());
    assert!(run(&mut task, Action::Extract, &Truncated, &mut |_| Ok(())).is_err());
    assert_eq!(task.coverage().done, 0);
    assert!(task.topic_cards.is_empty());
}

#[test]
fn changing_the_collection_block_size_does_not_invalidate_completed_topic_extraction() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.add_source(
        SourceSnapshot::new("长材料", SourceRole::Content, "原文段落。".repeat(500)).unwrap(),
    );
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("请拟定提纲") {
            "章节：工作要求 | 明确任务 | 500".into()
        } else {
            "无相关内容".into()
        }
    });
    run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap();
    task.chunk_bytes = 512;
    assert!(task.ready_for(Stage::Outline).is_ok());
    run(&mut task, Action::Plan, &model, &mut |_| Ok(())).unwrap();
    assert!(!task.sections.is_empty());
}

#[test]
fn tiny_or_corrupted_block_sizes_still_make_utf8_progress() {
    for size in [0, 1, 2, usize::MAX] {
        let original = "中文原文。跨块仍须完整。";
        let pieces = chunks(original, size);
        assert!(pieces.iter().all(|(start, end, _)| end > start));
        assert_eq!(
            pieces
                .into_iter()
                .map(|(_, _, text)| text)
                .collect::<String>(),
            original
        );
    }
}

#[test]
fn long_section_merge_uses_bounded_groups_and_can_resume_between_groups() {
    use crate::agent::backend::{Completion, ModelBackend, ModelRole};
    use crate::lmstudio::context::{Window, WindowSource, estimate_tokens};
    struct Small(ScriptedModel);
    impl ModelBackend for Small {
        fn complete(
            &self,
            role: ModelRole,
            system: &str,
            prompt: &str,
            delta: &mut dyn FnMut(crate::lmstudio::StreamDelta<'_>),
        ) -> anyhow::Result<Completion> {
            assert!(
                estimate_tokens(system) + estimate_tokens(prompt)
                    < crate::agent::budget::input_budget(8192),
                "归并请求必须适配小窗口"
            );
            self.0.complete(role, system, prompt, delta)
        }
        fn cancelled(&self) -> bool {
            false
        }
        fn window(&self, _: ModelRole) -> Window {
            Window {
                tokens: 8192,
                source: WindowSource::Manual,
            }
        }
    }
    let model = Small(ScriptedModel::new(|_, _| "保留任务要点并做好衔接。".into()));
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.style_md = "STYLE：明确克制".into();
    task.sections.push(Section {
        id: "s".into(),
        title: "任务要求".into(),
        markdown: "保留的原稿".into(),
        target_chars: 800,
        ..Default::default()
    });
    let pieces = vec!["需要归并的长段落原文。".repeat(250); 12];
    let first = execution::merge_section_parts(&mut task, 0, pieces.clone(), &model, &mut |task| {
        if task.sections[0]
            .merge_state
            .as_ref()
            .is_some_and(|state| state.next.len() == 2)
        {
            anyhow::bail!("模拟中断");
        }
        Ok(())
    });
    assert!(first.is_err());
    let before = model.0.prompts("归并第").len();
    assert_eq!(before, 2);
    let merged =
        execution::merge_section_parts(&mut task, 0, pieces, &model, &mut |_| Ok(())).unwrap();
    assert_eq!(merged, "保留任务要点并做好衔接。");
    assert!(model.0.prompts("STYLE").len() > 2);
    assert_eq!(task.sections[0].markdown, "保留的原稿");
}
