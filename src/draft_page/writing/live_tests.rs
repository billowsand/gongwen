//! 真机端到端：用真实模型把分阶段写作六个阶段整条跑一遍，默认忽略。
//!
//! 环境变量：
//! - `GONGWEN_LIVE_LLM_URL` / `GONGWEN_LIVE_LLM_MODEL` / `GONGWEN_LIVE_LLM_KEY`（可选）：模型服务；
//! - `GONGWEN_WRITING_TOPIC`：写作主题；
//! - `GONGWEN_WRITING_CONTENT`：内容材料路径，多份用 `;` 分隔；
//! - `GONGWEN_WRITING_STYLE`：风格样稿路径（可选，多份用 `;` 分隔）；
//! - `GONGWEN_WRITING_CHARS`：目标字数（缺省 3000）；
//! - `GONGWEN_WRITING_OUT`：输出目录，写入逐步保存的任务 JSON、运行日志、提纲与全文；
//! - `GONGWEN_WRITING_RESUME=1`：输出目录里已有 `task.json` 时从它续跑（与界面「接着跑」相同，
//!   已完成的块不再问模型）。
//!
//! 流程与界面一致：每个动作都走 `writing::run`，「确认提纲」由测试代替人点；块级失败先按
//! 「接着跑」续跑，仍失败的块在日志里列明后才人工跳过，不把失败算作成功。

use crate::agent::writing::{self, Action, ChunkStatus, SourceRole, SourceSnapshot, WritingTask};
use crate::models::{AppConfig, TemplateKind};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

struct Run {
    config: AppConfig,
    out: PathBuf,
    log: std::fs::File,
    started: Instant,
}

impl Run {
    fn log(&mut self, line: impl AsRef<str>) {
        let line = format!(
            "[{:>7.1}s] {}",
            self.started.elapsed().as_secs_f32(),
            line.as_ref()
        );
        eprintln!("{line}");
        let _ = writeln!(self.log, "{line}");
    }

    /// 跑一个动作；块级失败时按「接着跑」最多再续 `resumes` 次。
    fn step(
        &mut self,
        task: &mut WritingTask,
        action: Action,
        label: &str,
        resumes: usize,
    ) -> anyhow::Result<()> {
        let mut last = Ok(());
        for attempt in 0..=resumes {
            let backend = crate::agent::backend::LmBackend::new(
                &self.config,
                Arc::new(AtomicBool::new(false)),
            );
            let begun = Instant::now();
            let calls_before = task.usage.calls.len();
            let json = self.out.join("task.json");
            let mut statuses = Vec::new();
            let result = writing::run(task, action.clone(), &backend, &mut |task| {
                std::fs::write(&json, serde_json::to_vec_pretty(task)?)?;
                if statuses.last() != Some(&task.status) {
                    eprintln!("    · {}", task.status);
                    statuses.push(task.status.clone());
                }
                Ok(())
            });
            let calls = &task.usage.calls[calls_before.min(task.usage.calls.len())..];
            let (prompt, completion) = calls.iter().fold((0, 0), |(p, c), call| {
                (
                    p + call.tokens.prompt_tokens,
                    c + call.tokens.completion_tokens,
                )
            });
            for notice in backend.take_notices() {
                self.log(format!("  提示：{notice}"));
            }
            self.log(format!(
                "{label}{}：{}，{:.0} 秒，{} 次调用，输入 {prompt} / 输出 {completion} token",
                if attempt == 0 {
                    String::new()
                } else {
                    format!("（接着跑第 {attempt} 次）")
                },
                match &result {
                    Ok(()) => "完成".to_string(),
                    Err(error) => format!("失败：{error:#}"),
                },
                begun.elapsed().as_secs_f32(),
                calls.len(),
            ));
            last = result;
            if last.is_ok() || task.resume_action.as_ref() != Some(&action) {
                break;
            }
        }
        last
    }

    fn save(&self, name: &str, text: &str) {
        std::fs::write(self.out.join(name), text).expect("写输出文件");
    }
}

fn read_sources(paths: &str, role: SourceRole) -> Vec<SourceSnapshot> {
    paths
        .split(';')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {path}：{e}"));
            let title = Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path);
            SourceSnapshot::new(title, role, text).expect("材料快照")
        })
        .collect()
}

/// 列出仍失败的块，再按界面上的「人工确认无相关内容」处置，日志里如实记下。
fn skip_failed(run: &mut Run, task: &mut WritingTask) {
    let failed: Vec<_> = task
        .chunks
        .iter()
        .filter(|chunk| chunk.status == ChunkStatus::Failed && chunk.role == SourceRole::Content)
        .cloned()
        .collect();
    for chunk in failed {
        let Some(source) = task
            .sources
            .iter()
            .find(|source| source.id == chunk.source_id)
            .cloned()
        else {
            continue;
        };
        run.log(format!(
            "  仍失败：《{}》{}..{}（{}）：{}，人工跳过后继续",
            source.title,
            chunk.start,
            chunk.end,
            chunk
                .section_id
                .as_deref()
                .and_then(|id| task.sections.iter().find(|s| s.id == id))
                .map_or("主题提炼", |s| s.title.as_str()),
            chunk.error.as_deref().unwrap_or("无原因")
        ));
        task.mark_chunk_skipped(
            &source,
            chunk.section_id.as_deref(),
            &chunk.query,
            chunk.start,
            chunk.end,
        );
    }
}

#[test]
#[ignore = "需要真实模型与本机材料"]
fn live_staged_writing_end_to_end() {
    let (Ok(url), Ok(model), Ok(topic), Ok(content), Ok(out)) = (
        std::env::var("GONGWEN_LIVE_LLM_URL"),
        std::env::var("GONGWEN_LIVE_LLM_MODEL"),
        std::env::var("GONGWEN_WRITING_TOPIC"),
        std::env::var("GONGWEN_WRITING_CONTENT"),
        std::env::var("GONGWEN_WRITING_OUT"),
    ) else {
        eprintln!("未设置模型或材料环境变量，跳过");
        return;
    };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).expect("建输出目录");
    let mut config = AppConfig::default();
    config.lm_studio.base_url = url;
    config.lm_studio.model = model.clone();
    config.lm_studio.api_key = std::env::var("GONGWEN_LIVE_LLM_KEY").unwrap_or_default();
    config.lm_studio.timeout_seconds = 600;
    let mut run = Run {
        config,
        log: std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out.join("run.log"))
            .expect("建日志"),
        out,
        started: Instant::now(),
    };

    let saved = run.out.join("task.json");
    if std::env::var("GONGWEN_WRITING_RESUME").as_deref() == Ok("1") && saved.exists() {
        let mut task: WritingTask =
            serde_json::from_slice(&std::fs::read(&saved).expect("读任务")).expect("解析任务");
        task.adopt_legacy_records();
        // 上次是测试替人做的跳过：续跑时撤销，让这些块按当前代码重新提炼。
        let skipped: Vec<_> = task
            .chunks
            .iter()
            .filter(|chunk| chunk.status == ChunkStatus::SkippedByUser)
            .cloned()
            .collect();
        for chunk in skipped {
            if let Some(source) = task
                .sources
                .iter()
                .find(|source| source.id == chunk.source_id)
                .cloned()
            {
                task.unskip_chunk(
                    &source,
                    chunk.section_id.as_deref(),
                    &chunk.query,
                    chunk.start,
                    chunk.end,
                );
            }
        }
        run.log(format!(
            "从上次保存的任务续跑：阶段 {:?}，已有块记录 {} 条",
            task.stage,
            task.chunks.len()
        ));
        return finish(run, task, model);
    }
    let mut task = WritingTask::new(TemplateKind::ResearchReport);
    task.topic = topic;
    task.target_chars = std::env::var("GONGWEN_WRITING_CHARS")
        .ok()
        .and_then(|chars| chars.parse().ok())
        .unwrap_or(3000);
    for source in read_sources(&content, SourceRole::Content)
        .into_iter()
        .chain(read_sources(
            &std::env::var("GONGWEN_WRITING_STYLE").unwrap_or_default(),
            SourceRole::Style,
        ))
    {
        run.log(format!(
            "材料：{}《{}》{} 字",
            source.role.label(),
            source.title,
            source.text.chars().count()
        ));
        task.add_source(source);
    }
    finish(run, task, model);
}

fn finish(mut run: Run, mut task: WritingTask, model: String) {
    run.log(format!(
        "模型 {model}；主题「{}」；目标 {} 字",
        task.topic, task.target_chars
    ));

    // 一、二：提炼素材。
    if run.step(&mut task, Action::Extract, "提炼素材", 2).is_err() {
        skip_failed(&mut run, &mut task);
    }
    let coverage = task.coverage();
    run.log(format!(
        "  覆盖：{}/{} 块（人工跳过 {}，失败 {}）；主题卡片 {} 条",
        coverage.done,
        coverage.total,
        coverage.skipped,
        coverage.failed,
        task.current_topic_cards().len()
    ));
    run.save("style.md", &task.style_md);
    run.save("framework.md", &task.framework_md);

    // 三：拟纲，人工确认。
    run.step(&mut task, Action::Plan, "生成提纲", 1)
        .expect("提纲生成失败");
    let outline: String = task
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}. {}（{} 字）：{}\n",
                i + 1,
                s.title,
                s.target_chars,
                s.points
            )
        })
        .collect();
    run.log(format!("  提纲 {} 节：\n{outline}", task.sections.len()));
    run.save("outline.md", &outline);
    task.confirm_outline().expect("确认提纲");

    // 四：分节备料。
    if run.step(&mut task, Action::Collect, "分节备料", 2).is_err() {
        skip_failed(&mut run, &mut task);
    }
    for section in task.sections.clone() {
        let coverage = task.section_coverage(&section.id);
        run.log(format!(
            "  {}：素材 {} 条，块 {}/{}（跳过 {}，失败 {}）",
            section.title,
            task.current_section_cards(&section).len(),
            coverage.done,
            coverage.total,
            coverage.skipped,
            coverage.failed
        ));
    }

    // 五：分节写作。
    run.step(&mut task, Action::Write, "分节写作", 2)
        .expect("分节写作失败");
    for section in &task.sections {
        run.log(format!(
            "  {}：{} 字 / 目标 {} 字",
            section.title,
            section.markdown.chars().count(),
            section.target_chars
        ));
    }

    // 六：统稿。
    let polished = run.step(&mut task, Action::Polish, "统稿", 1);
    run.save("final.md", &task.final_markdown);
    run.save("review-notes.md", &task.review_notes);
    run.log(format!(
        "  全文 {} 字；统稿{}",
        task.final_markdown.chars().count(),
        if polished.is_ok() {
            "完成"
        } else {
            "未通过"
        }
    ));

    // 交付前检查：与「审阅并合并到正文…」走同一套确定性检查，不真正写正文。
    let issues = super::validation::delivery_issues(&task, &run.config.vocabulary);
    run.log(format!("交付前检查 {} 条：", issues.len()));
    for issue in &issues {
        run.log(format!("  - {issue}"));
    }
    let draft = crate::models::DraftInput {
        kind: TemplateKind::ResearchReport,
        ..Default::default()
    };
    let reviewed =
        crate::draft_page::reviewed_draft(&draft, &run.config, &task.final_markdown, false);
    run.save("proposal.md", &reviewed.markdown);
    run.log(format!(
        "合并提案：题名「{}」，{} 条提示",
        reviewed.title,
        reviewed.warnings.len()
    ));
    for warning in reviewed.warnings.iter().take(30) {
        run.log(format!("  - {}", warning.message));
    }
    let headings = crate::export::parse_markdown(&reviewed.markdown)
        .iter()
        .filter(|block| matches!(block, crate::export::MarkdownBlock::Heading(..)))
        .count();
    let calls = task.usage.calls.len();
    let elapsed: u64 = task.usage.calls.iter().map(|call| call.elapsed_ms).sum();
    run.log(format!(
        "合计：最后一轮动作记账 {calls} 次调用、模型耗时 {:.0} 秒；正文标题 {headings} 个；总用时 {:.0} 秒",
        elapsed as f64 / 1000.0,
        run.started.elapsed().as_secs_f32()
    ));
    assert!(
        !task.final_markdown.trim().is_empty(),
        "整条流程应当产出统稿全文"
    );
}
