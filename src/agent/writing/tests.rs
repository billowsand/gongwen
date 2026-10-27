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

/// 跑到分节写作完成：两节、一份内容材料，模型按提示词固定作答。
fn written_task() -> (WritingTask, ScriptedModel) {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "基层减负".into();
    task.add_source(
        SourceSnapshot::new("材料", SourceRole::Content, "合并同类报表。规范检查考核。").unwrap(),
    );
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("请拟定提纲") || prompt.contains("请调整提纲") {
            // 模型常常不听话手写编号，拼接时要剥掉。
            "章节：一、精简报表 | 合并报表 | 300字\n章节：二、规范考核 | 统筹检查 | 300字".into()
        } else if prompt.contains("当前章节素材组") {
            // 文种规则让模型用 `##` 写一级标题，节内小标题会与节标题同级。
            "## 具体措施\n\n现已合并同类报表。".into()
        } else if prompt.contains("正文片段") {
            fragment_of(prompt).unwrap_or_default()
        } else if prompt.contains("原文块") || prompt.contains("本块位置") {
            "观点：合并报表\n原句：合并同类报表。".into()
        } else {
            "未发现明显结构问题".into()
        }
    });
    let mut save = |_: &WritingTask| Ok(());
    run(&mut task, Action::Extract, &model, &mut save).unwrap();
    run(&mut task, Action::Plan, &model, &mut save).unwrap();
    task.confirm_outline().unwrap();
    run(&mut task, Action::Collect, &model, &mut save).unwrap();
    run(&mut task, Action::Write, &model, &mut save).unwrap();
    (task, model)
}

/// 统稿提示词里的正文片段原样返回，模拟只改措辞不改分段的统稿。
fn fragment_of(prompt: &str) -> Option<String> {
    let start = prompt.find("正文片段")?;
    let body = &prompt[start..];
    let body = &body[body.find("：\n")? + "：\n".len()..];
    Some(body[..body.find("\n片段相邻上下文")?].to_string())
}

#[test]
fn topic_edits_keep_finished_extraction_until_replaced_and_reverting_restores_it() {
    let (mut task, _) = written_task();
    let coverage = task.coverage();
    assert_eq!(coverage.done, coverage.total);
    let first = task.sections[0].id.clone();
    let cards = task.current_section_cards(&task.sections[0]).len();
    assert!(cards > 0);

    task.set_topic("另一个主题".into());
    assert_eq!(task.coverage().done, 0, "新主题下旧提炼不算当前成果");
    assert!(task.current_topic_cards().is_empty());
    assert_eq!(task.section_coverage(&first).done, 0);
    assert!(!task.outline_confirmed);
    assert!(!task.chunks.is_empty(), "改主题不删除已花调用的成果");

    task.set_topic("基层减负".into());
    assert_eq!(task.coverage().done, coverage.total, "改回原主题即恢复");
    assert_eq!(task.current_section_cards(&task.sections[0]).len(), cards);
}

#[test]
fn adding_or_removing_one_material_keeps_the_others_extraction() {
    let (mut task, _) = written_task();
    let content = task.coverage().done;
    task.add_source(SourceSnapshot::new("风格", SourceRole::Style, "先结论，后措施。").unwrap());
    assert_eq!(task.coverage().done, content, "加风格样稿不重读内容材料");
    let first = task.sections[0].id.clone();
    assert_eq!(
        task.section_coverage(&first).done,
        task.section_coverage(&first).total
    );

    let extra = SourceSnapshot::new("补充", SourceRole::Content, "另有补充事实。").unwrap();
    let extra_id = extra.id.clone();
    task.add_source(extra);
    assert_eq!(task.coverage().done, content, "原有材料的块照常复用");
    assert_eq!(task.coverage().pending, 1);
    assert!(!task.outline_confirmed);

    let original = task.sources[0].id.clone();
    assert!(task.source_impact(&original).0 > 0);
    assert!(task.remove_source(&original));
    assert!(task.chunks.iter().all(|chunk| chunk.source_id != original));
    assert!(
        task.topic_cards
            .iter()
            .chain(task.sections.iter().flat_map(|s| s.cards.iter()))
            .all(|card| card.source_id != original)
    );
    assert_eq!(task.source_impact(&extra_id), (0, 0));
}

#[test]
fn outline_reordering_keeps_section_material_and_an_edit_only_affects_its_section() {
    let (mut task, _) = written_task();
    let (first, second) = (task.sections[0].id.clone(), task.sections[1].id.clone());
    task.sections.swap(0, 1);
    task.outline_changed();
    assert!(!task.outline_confirmed);
    task.confirm_outline().unwrap();
    assert!(
        task.ready_for(Stage::Write).is_ok(),
        "只调顺序不需要重新备料"
    );

    let index = task.sections.iter().position(|s| s.id == second).unwrap();
    task.sections[index].points.push_str("，补充时限");
    task.outline_changed();
    assert_eq!(task.section_coverage(&second).done, 0);
    let covered = task.section_coverage(&first);
    assert_eq!(covered.done, covered.total, "未改动的章节沿用已备素材");
    assert!(task.sections.iter().all(|s| !s.markdown.is_empty()));
}

#[test]
fn style_edits_do_not_send_material_back_through_the_model() {
    let (mut task, model) = written_task();
    let reads = model.prompts("本块位置").len();
    task.style_md = "改过的写法：句子更短。".into();
    task.style_changed(true);
    assert!(task.style_manual);
    assert!(task.ready_for(Stage::Write).is_ok());
    run(&mut task, Action::Write, &model, &mut |_| Ok(())).unwrap();
    assert_eq!(model.prompts("本块位置").len(), reads, "改风格不重读材料");
    assert!(
        model
            .prompts("改过的写法")
            .iter()
            .any(|p| p.contains("当前章节素材组")),
        "未经人工修改的章节按新风格重写"
    );
}

#[test]
fn rewriting_a_hand_edited_section_returns_it_to_batch_writing_and_review() {
    let (mut task, model) = written_task();
    task.sections[0].markdown = "人工改过的正文。".into();
    task.sections[0].manually_edited = true;
    run(
        &mut task,
        Action::RewriteSection(0, "更简洁".into()),
        &model,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert!(!task.sections[0].manually_edited);
    assert_eq!(task.sections[0].revision_history, ["人工改过的正文。"]);
    run(&mut task, Action::Polish, &model, &mut |_| Ok(())).unwrap();
    assert!(
        task.sections[0].review_suggestion.is_some(),
        "重写后的章节参加统稿"
    );
}

#[test]
fn assembled_draft_keeps_section_titles_above_body_headings_without_manual_numbers() {
    let (mut task, model) = written_task();
    assert_eq!(task.sections[0].title, "一、精简报表");
    run(&mut task, Action::Polish, &model, &mut |_| Ok(())).unwrap();
    assert!(
        task.final_markdown
            .starts_with("## 精简报表\n\n### 具体措施\n\n现已合并同类报表。"),
        "{}",
        task.final_markdown
    );
    assert!(
        task.final_markdown
            .contains("\n\n## 规范考核\n\n### 具体措施")
    );
}

#[test]
fn polishing_a_long_section_never_breaks_a_paragraph_mid_sentence() {
    let paragraph = "各单位要统筹安排检查事项，避免多头重复。".repeat(120);
    let text = format!("{paragraph}\n\n第二段说明。\n| 项目 | 时限 |\n|---|---|\n| 报表 | 本月 |");
    let parts = execution::polish_parts(&text, 512);
    assert!(parts.len() > 2);
    for part in &parts[..parts.len() - 1] {
        assert!(
            part.text.ends_with('。') || part.text.ends_with('|'),
            "片段只能在句末或段末结束：{}",
            part.text
        );
    }
    assert!(parts.iter().any(|part| {
        part.text
            .contains("| 项目 | 时限 |\n|---|---|\n| 报表 | 本月 |")
    }));
    let same: Vec<String> = parts.iter().map(|part| part.text.clone()).collect();
    assert_eq!(execution::join_polished(&parts, &same), text);

    let (mut task, model) = written_task();
    task.sections[0].markdown = paragraph.clone();
    task.drafts_changed();
    task.chunk_bytes = 512;
    run(&mut task, Action::Polish, &model, &mut |_| Ok(())).unwrap();
    assert_eq!(
        task.sections[0].review_suggestion.as_deref(),
        Some(paragraph.as_str())
    );
}

#[test]
fn hand_edits_to_the_final_draft_survive_upstream_changes_and_are_archived_on_repolish() {
    let (mut task, model) = written_task();
    run(&mut task, Action::Polish, &model, &mut |_| Ok(())).unwrap();
    task.final_markdown.push_str("\n\n人工补充的结语。");
    task.final_manual = true;
    let edited = task.final_markdown.clone();

    task.sections[1].locked = true;
    task.drafts_changed();
    assert_eq!(task.final_markdown, edited, "人工统稿不被上游变化清空");
    assert!(task.stale_reason.is_some());

    run(&mut task, Action::Polish, &model, &mut |_| Ok(())).unwrap();
    assert!(!task.final_manual);
    assert_eq!(task.final_history, std::slice::from_ref(&edited));
    assert!(!task.final_markdown.contains("人工补充的结语"));
    task.restore_final(0);
    assert_eq!(task.final_markdown, edited);
    assert!(task.final_manual);

    task.final_manual = false;
    task.drafts_changed();
    assert!(
        task.final_markdown.is_empty(),
        "未经人工修改的统稿随上游失效"
    );
}

#[test]
fn fixed_skeleton_kinds_cannot_run_staged_writing() {
    for kind in [
        TemplateKind::MeetingAgenda,
        TemplateKind::MeetingMinutes,
        TemplateKind::PhoneRecord,
    ] {
        assert!(!supports(kind));
        let mut task = WritingTask::new(kind);
        task.topic = "会议".into();
        let model = ScriptedModel::new(|_, _| "无相关内容".into());
        let error = run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap_err();
        assert!(error.to_string().contains("不适合分阶段写作"));
        assert!(model.prompts("").is_empty());
    }
    assert!(supports(TemplateKind::PlainDocument));
    assert!(supports(TemplateKind::ResearchReport));
}

#[test]
fn records_saved_before_fingerprints_are_adopted_as_current() {
    let (mut task, _) = written_task();
    let done = task.coverage().done;
    for chunk in &mut task.chunks {
        chunk.query.clear();
    }
    for card in task
        .topic_cards
        .iter_mut()
        .chain(task.sections.iter_mut().flat_map(|s| s.cards.iter_mut()))
    {
        card.query.clear();
    }
    assert_eq!(task.coverage().done, 0);
    task.adopt_legacy_records();
    assert_eq!(task.coverage().done, done);
    assert!(task.ready_for(Stage::Review).is_ok());
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
