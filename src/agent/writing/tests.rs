//! 分阶段写作的流程与来源回归测试。

use super::execution::chunks;
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
        } else if prompt.contains("本节为锁定或人工稿") {
            "建议改法。".into()
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

#[test]
fn a_failed_block_does_not_stop_the_rest_of_extraction() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    for (title, text) in [
        ("甲材料", "甲材料原文。"),
        ("乙材料", "乙材料原文。"),
        ("丙材料", "丙材料原文。"),
    ] {
        task.add_source(SourceSnapshot::new(title, SourceRole::Content, text).unwrap());
    }
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("乙材料原文") {
            "观点：伪造观点\n原句：不存在的原句。".into()
        } else {
            "无相关内容".into()
        }
    });
    let error = run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap_err();
    assert!(error.to_string().contains("1 块失败"), "{error:#}");
    let coverage = task.coverage();
    assert_eq!(coverage.total, 3);
    assert_eq!(coverage.failed, 1);
    assert_eq!(coverage.done, 2, "失败块后面的材料仍被处理");
    assert_eq!(
        model.prompts("丙材料原文").len(),
        1,
        "失败块后面的材料仍被处理"
    );
    assert_eq!(task.resume_action, Some(Action::Extract));

    // 接着跑只重试失败的那一块。
    let retry = ScriptedModel::new(|_, _| "无相关内容".into());
    run(&mut task, Action::Extract, &retry, &mut |_| Ok(())).unwrap();
    assert_eq!(retry.prompts("乙材料原文").len(), 1);
    assert_eq!(retry.prompts("甲材料原文").len(), 0, "已成功的块不重跑");
    assert_eq!(task.coverage().done, 3);
    assert_eq!(task.resume_action, None);
}

#[test]
fn a_manually_skipped_block_counts_as_covered_and_can_be_undone() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    let source = SourceSnapshot::new("材料", SourceRole::Content, "原文。").unwrap();
    task.add_source(source.clone());
    let model = ScriptedModel::new(|_, _| "观点：假观点\n原句：不存在的原句。".into());
    assert!(run(&mut task, Action::Extract, &model, &mut |_| Ok(())).is_err());
    assert_eq!(task.coverage().failed, 1);
    let query = task.topic_query();

    assert!(task.mark_chunk_skipped(&source, None, &query, 0, source.text.len()));
    let coverage = task.coverage();
    assert_eq!(coverage.done, 1);
    assert_eq!(coverage.skipped, 1);
    assert_eq!(coverage.failed, 0);
    assert!(
        task.ready_for(Stage::Outline).is_ok(),
        "人工跳过计入覆盖，可以继续"
    );
    assert_eq!(
        task.chunks[0].status,
        ChunkStatus::SkippedByUser,
        "不能记成模型判定的无相关内容"
    );
    assert_ne!(task.chunks[0].status, ChunkStatus::NoRelevantContent);

    assert!(task.unskip_chunk(&source, None, &query, 0, source.text.len()));
    let coverage = task.coverage();
    assert_eq!(coverage.done, 0);
    assert_eq!(coverage.skipped, 0);
    assert_eq!(coverage.pending, 1);
    assert!(task.ready_for(Stage::Outline).is_err());
}

#[test]
fn cancelling_stops_immediately_instead_of_retrying_the_rest() {
    use crate::agent::backend::{Completion, ModelBackend, ModelRole};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Cancelling {
        inner: ScriptedModel,
        cancelled: AtomicBool,
        calls: AtomicUsize,
    }
    impl ModelBackend for Cancelling {
        fn complete(
            &self,
            role: ModelRole,
            system: &str,
            prompt: &str,
            delta: &mut dyn FnMut(crate::lmstudio::StreamDelta<'_>),
        ) -> anyhow::Result<Completion> {
            let answer = self.inner.complete(role, system, prompt, delta);
            if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
                // 第一块之后用户点了暂停。
                self.cancelled.store(true, Ordering::Relaxed);
            }
            answer
        }
        fn cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Relaxed)
        }
    }

    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.add_source(
        SourceSnapshot::new("长材料", SourceRole::Content, "原文段落。".repeat(500)).unwrap(),
    );
    let model = Cancelling {
        inner: ScriptedModel::new(|_, _| "无相关内容".into()),
        cancelled: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    };
    let error = run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap_err();
    assert!(error.to_string().contains("已暂停"), "{error:#}");
    let coverage = task.coverage();
    assert!(coverage.total > 1);
    assert_eq!(coverage.done, 1, "取消后不再安排新请求");
    assert_eq!(coverage.failed, 0, "取消不是块失败");
    assert_eq!(model.inner.prompts("原文块").len(), 1);
}

#[test]
fn locked_sections_get_advisory_suggestions_but_the_assembled_text_keeps_the_draft() {
    let (mut task, model) = written_task();
    task.sections[0].locked = true;
    let original = task.sections[0].markdown.clone();
    run(&mut task, Action::Polish, &model, &mut |_| Ok(())).unwrap();
    let section = &task.sections[0];
    assert!(
        section.advisory_suggestion.is_some(),
        "锁定节也要收到统稿建议"
    );
    assert_eq!(section.markdown, original, "建议不能直接覆盖原稿");
    assert!(task.final_markdown.contains(&original));
    assert!(
        !task.final_markdown.contains("建议改法。"),
        "拼全文仍用本节正文"
    );

    assert!(task.accept_advisory(0));
    assert_eq!(task.sections[0].markdown, "建议改法。");
    assert_eq!(
        task.sections[0].revision_history.last(),
        Some(&original),
        "采纳前正文先进本节历史稿"
    );
    assert!(task.sections[0].manually_edited, "采纳后仍是人工稿");
    assert!(task.final_markdown.contains("建议改法。"), "全文随采纳更新");
}

#[test]
fn switching_document_kind_updates_default_style_and_keeps_manual_ones() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.style_md = default_style(TemplateKind::PlainDocument);
    task.framework_md = default_framework(TemplateKind::PlainDocument);
    let before = task.style_md.clone();
    assert!(task.sync_kind(TemplateKind::OfficialLetter));
    assert_ne!(task.style_md, before, "默认写法随文种更新");
    assert_eq!(task.style_md, default_style(TemplateKind::OfficialLetter));
    assert!(task.stale_reason.as_deref().unwrap().contains("文种"));
    assert!(!task.outline_confirmed);

    let mut manual = WritingTask::new(TemplateKind::PlainDocument);
    manual.style_md = "我手改的风格".into();
    manual.style_manual = true;
    manual.sync_kind(TemplateKind::ResearchReport);
    assert_eq!(manual.style_md, "我手改的风格", "人工改过的不覆盖");
}

#[test]
fn missing_section_chars_are_split_by_parsed_section_count_not_output_lines() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.target_chars = 900;
    task.add_source(SourceSnapshot::new("材料", SourceRole::Content, "原文。").unwrap());
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("请拟定提纲") {
            "先说明一句。\n\n章节：甲 | 要点一 | \n章节：乙 | 要点二\n章节：丙 | 要点三\n\n结尾说明。"
                .into()
        } else {
            "无相关内容".into()
        }
    });
    run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap();
    run(&mut task, Action::Plan, &model, &mut |_| Ok(())).unwrap();
    assert_eq!(task.sections.len(), 3);
    assert!(
        task.sections
            .iter()
            .all(|section| section.target_chars == 300),
        "按解析出的 3 节平分 900 字，而不是按输出行数"
    );
}

#[test]
fn revised_outline_keeps_section_identity_for_renamed_locked_sections() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.add_source(SourceSnapshot::new("材料", SourceRole::Content, "原文。").unwrap());
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("请调整提纲") {
            "章节：#2 乙节 | 乙要点 | 300字\n章节：#1 新题目 | 新要点 | 300字".into()
        } else {
            "无相关内容".into()
        }
    });
    run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap();
    let mut locked = Section {
        id: "甲".into(),
        title: "旧题目".into(),
        markdown: "人工旧正文。".into(),
        locked: true,
        ..Default::default()
    };
    locked.manually_edited = true;
    task.sections = vec![
        locked,
        Section {
            id: "乙".into(),
            title: "乙节".into(),
            markdown: "乙正文。".into(),
            ..Default::default()
        },
    ];
    run(
        &mut task,
        Action::ReviseOutline("调整顺序".into()),
        &model,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(task.sections.len(), 2);
    let renamed = task
        .sections
        .iter()
        .find(|section| section.id == "甲")
        .expect("改名后仍保留章节身份");
    assert_eq!(renamed.title, "新题目");
    assert_eq!(renamed.markdown, "人工旧正文。");
    assert!(renamed.locked);
    assert_eq!(
        task.sections
            .iter()
            .find(|section| section.id == "乙")
            .map(|section| section.markdown.as_str()),
        Some("乙正文。")
    );
    assert!(
        !task
            .sections
            .iter()
            .any(|section| section.title == "旧题目")
    );
}

#[test]
fn a_locked_section_dropped_by_the_model_stays_in_the_outline_with_a_warning() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.add_source(SourceSnapshot::new("材料", SourceRole::Content, "原文。").unwrap());
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("请调整提纲") {
            "章节：#2 乙节 | 乙要点 | 300字".into()
        } else {
            "无相关内容".into()
        }
    });
    run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap();
    task.sections = vec![
        Section {
            id: "甲".into(),
            title: "锁定章节".into(),
            markdown: "人工旧正文。".into(),
            locked: true,
            ..Default::default()
        },
        Section {
            id: "乙".into(),
            title: "乙节".into(),
            markdown: "乙正文。".into(),
            ..Default::default()
        },
    ];
    run(
        &mut task,
        Action::ReviseOutline("精简提纲".into()),
        &model,
        &mut |_| Ok(()),
    )
    .unwrap();
    let retained = task
        .sections
        .iter()
        .find(|section| section.id == "甲")
        .expect("锁定章节不得静默丢弃");
    assert_eq!(retained.markdown, "人工旧正文。");
    assert!(task.stale_reason.as_deref().unwrap().contains("锁定章节"));
}

#[test]
fn manually_edited_style_skips_the_summarization_call() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    task.style_md = "人工风格：短句。".into();
    task.style_manual = true;
    task.add_source(SourceSnapshot::new("材料", SourceRole::Content, "原文。").unwrap());
    task.add_source(SourceSnapshot::new("样稿", SourceRole::Style, "样稿原文。").unwrap());
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("样稿原文") || prompt.contains("样稿：") {
            "风格分析。".into()
        } else {
            "无相关内容".into()
        }
    });
    run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap();
    assert_eq!(task.style_md, "人工风格：短句。");
    assert_eq!(model.asked("归纳层级"), 0, "人工改过的风格不再调模型归纳");
}

#[test]
fn the_topic_directory_is_summarized_once_until_its_input_changes() {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "工作".into();
    let source = SourceSnapshot::new("材料", SourceRole::Content, "材料原文。").unwrap();
    task.add_source(source.clone());
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("请拟定提纲") || prompt.contains("请调整提纲") {
            "章节：甲 | 要点 | 500字".into()
        } else if prompt.contains("目录整理层级") {
            "主题观点目录。".into()
        } else {
            "无相关内容".into()
        }
    });
    run(&mut task, Action::Extract, &model, &mut |_| Ok(())).unwrap();
    task.topic_cards.push(MaterialCard {
        source_id: source.id.clone(),
        quote: source.text.clone(),
        point: "已提炼观点".into(),
        start: 0,
        end: source.text.len(),
        query: task.topic_query(),
        ..Default::default()
    });
    run(&mut task, Action::Plan, &model, &mut |_| Ok(())).unwrap();
    assert_eq!(model.asked("目录整理层级"), 1);
    run(&mut task, Action::Plan, &model, &mut |_| Ok(())).unwrap();
    assert_eq!(
        model.asked("目录整理层级"),
        1,
        "输入未变时连续调整提纲只归纳一次目录"
    );
    assert_eq!(model.asked("请拟定提纲"), 2);
}

#[test]
fn section_statuses_from_old_task_json_are_still_readable() {
    assert_eq!(SectionStatus::default(), SectionStatus::Idle);
    for (raw, expected) in [
        ("", SectionStatus::Idle),
        ("备料进行中", SectionStatus::Collecting),
        ("备料完成", SectionStatus::Collected),
        ("写作中", SectionStatus::Writing),
        ("已完成", SectionStatus::Done),
        ("人工已完成", SectionStatus::ManualDone),
        ("已有统稿建议，待审阅", SectionStatus::ReviewReady),
    ] {
        let status: SectionStatus = serde_json::from_str(&format!("\"{raw}\"")).unwrap();
        assert_eq!(status, expected, "{raw}");
        assert_eq!(
            serde_json::to_string(&expected).unwrap(),
            format!("\"{raw}\"")
        );
    }
}

#[test]
fn a_task_json_saved_before_the_status_enum_still_loads() {
    // 与 v0.9.9 / fdfd0c8 存下的结构一致：没有本轮新增字段，章节状态是中文串。
    let raw = serde_json::json!({
        "id": "task-1",
        "kind": "PlainDocument",
        "topic": "基层减负",
        "target_chars": 1000,
        "stage": "Write",
        "sources": [{
            "id": "src-1",
            "title": "材料",
            "role": "Content",
            "text": "合并同类报表。",
            "hash": digest("合并同类报表。".as_bytes()),
        }],
        "style_md": "风格。",
        "framework_md": "框架。",
        "topic_cards": [],
        "sections": [
            {
                "id": "a",
                "title": "第一节",
                "points": "要点",
                "target_chars": 300,
                "markdown": "正文。",
                "locked": false,
                "cards": [],
                "status": "已完成",
            },
            {
                "id": "b",
                "title": "第二节",
                "points": "要点",
                "target_chars": 300,
                "markdown": "",
                "locked": false,
                "cards": [],
                "status": "备料完成",
            },
            {
                "id": "c",
                "title": "第三节",
                "points": "要点",
                "target_chars": 300,
                "markdown": "人工正文。",
                "locked": true,
                "cards": [],
                "status": "人工已完成",
            }
        ],
        "outline_confirmed": true,
        "final_markdown": "",
        "status": "统稿进行中",
        "version": 1,
    });
    let task: WritingTask = serde_json::from_value(raw).unwrap();
    assert_eq!(task.version, 1);
    assert_eq!(task.sections[0].status, SectionStatus::Done);
    assert_eq!(task.sections[1].status, SectionStatus::Collected);
    assert_eq!(task.sections[2].status, SectionStatus::ManualDone);
    assert_eq!(task.sections[2].status.label(), "人工已完成");
    assert!(task.style_summary_input.is_empty());
    assert!(task.sections[0].advisory_suggestion.is_none());
    assert_eq!(task.stage, Stage::Write);
    let written = serde_json::to_value(&task).unwrap();
    assert_eq!(written["sections"][0]["status"], "已完成");
}

#[test]
fn card_verification_three_states_are_counted_and_kept_out_of_the_hard_gate() {
    let (mut task, _) = written_task();
    let total: usize = task
        .sections
        .iter()
        .map(|section| task.current_section_cards(section).len())
        .sum();
    assert_eq!(total, 2);
    assert_eq!(task.verification_stats().unverified, total);
    task.sections[0].cards[0].verification = Verification::Conflict;
    task.sections[1].cards[0].verification = Verification::Insufficient;
    let stats = task.verification_stats();
    assert_eq!(stats.conflict, 1);
    assert_eq!(stats.insufficient, 1);
    assert_eq!(stats.checked, 0);
    assert_eq!(stats.unverified, 0);

    let issues = task.verification_issue_cards();
    assert_eq!(issues.len(), 2, "冲突与依据不足单独列出");
    assert!(issues.iter().any(|(title, card)| {
        !title.is_empty()
            && card.verification == Verification::Conflict
            && card.quote == "合并同类报表。"
    }));

    // 软提醒不进入交付硬检查：验证状态本身不产生 delivery issue。
    let before = task.verification_stats();
    task.sections[0].cards[0].verification = Verification::Unverified;
    let after = task.verification_stats();
    assert_eq!(before.conflict - after.conflict, 1);
}
