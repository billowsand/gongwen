//! Pi 智能写作结果交付与异步任务隔离测试。
//!
//! 产品契约（2026-11-02 用户确认，`docs/pi-implementation/pi-direct-write-task.md`）：Pi 明确
//! 提交的有效 Markdown 由界面线程**直接写进当前可编辑正文**，普通流程不再弹人工校订窗、
//! 不勾事实、不接受提案。新任务不执行事实核查、不生成事实提示；
//! 路径 / 哈希 / 体积 / 选区 / 只读 / 基线这些技术性与权限保护照旧拦下。
//! 2026-11-04 修订：被拦的成品不再丢弃——留在轮次里等用户显式「取回 AI 成品」（写入
//! 同样可撤销）或丢弃；「接着写」不再重放同一份被拒回执，想再写只能从头开一轮。
//! 锁定选区的任务里，写入前的确定性规整只许改动选区内的字节，选区外一律还原成
//! 模型提交的原文（`pi_agent::restore_outside_selection`）。
//! 报告型产物仍然只进侧栏。

use super::GongwenApp;
use crate::ai_panel::TurnState;
use crate::models::ReviewNote;
use crate::pi_agent::{DeliveryCheck, DeliveryReport, PiDelivery, PiOutcome};

/// 一份 Pi 交付的结构化检查结果。原稿与规整后的稿件各跑一遍，取并集。
fn delivery_check(
    config: &crate::models::AppConfig,
    delivery: &PiDelivery,
    content: &str,
) -> DeliveryReport {
    crate::pi_agent::check_delivery(&DeliveryCheck {
        before: &delivery.before,
        base: &delivery.base,
        request: &delivery.request,
        content,
        selection: delivery.selection,
        fact_sources: &delivery.fact_sources,
        vocabulary: &config.vocabulary,
    })
}

impl GongwenApp {
    /// Pi 本轮结束：正文交付直接写入当前正文（可用卡片或 Ctrl+Z 撤销），
    /// 侧栏清单只进侧栏，失败只留原因，绝不动正文。
    pub(super) fn apply_pi_done(
        &mut self,
        index: usize,
        prefix: &str,
        ctx: Option<&eframe::egui::Context>,
        result: Result<PiOutcome, String>,
    ) {
        let doc = &mut self.docs[index];
        doc.ai_panel.pi_ask = None;
        doc.ai_panel.pi_bridge = None;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                doc.ai_review_baseline = None;
                doc.ai_panel.finish(TurnState::Failed(error.clone()));
                self.status = format!("{prefix}Pi 智能写作未完成：{error}");
                return;
            }
        };
        match outcome {
            PiOutcome::Style(delivery) => {
                doc.ai_review_baseline = None;
                let reason = format!(
                    "收到独立风格文件 {}，正文未改变。请从「AI 管理 → 风格」学习和保存风格。",
                    delivery.path
                );
                doc.ai_panel.finish(TurnState::Failed(reason.clone()));
                self.status = reason;
            }
            PiOutcome::Report(report) => {
                let notes = report.confirmed;
                if let Some(turn) = doc.ai_panel.running_turn_mut() {
                    turn.elapsed = Some(report.elapsed);
                    if let Some(run) = turn.pi_run.as_mut() {
                        run.status = crate::pi_agent::PiRunStatus::Delivered;
                        run.sources = report.sources_sha256.clone();
                        run.updated_at = chrono::Local::now().to_rfc3339();
                    }
                    if report.recovered {
                        turn.notes
                            .push("这是从上次运行的交付回执恢复的清单，没有重新生成。".into());
                    }
                    // 完整报告进入已有的会话正文持久化路径，换轮或重启后仍可回看。
                    turn.is_workspace = false;
                    turn.content = report.summary.clone();
                    for item in &report.items {
                        turn.content.push_str(&format!(
                            "\n\n- ［{}］{}：{}",
                            item.severity, item.title, item.detail
                        ));
                    }
                    for note in notes {
                        turn.notes.push(format!("已确认：{note}"));
                    }
                }
                let items = report.items.len();
                for item in &report.items {
                    if let Some(turn) = doc.ai_panel.running_turn_mut() {
                        turn.notes.push(if item.detail.trim().is_empty() {
                            format!("［{}］{}", item.severity, item.title)
                        } else {
                            format!("［{}］{}：{}", item.severity, item.title, item.detail)
                        });
                    }
                }
                doc.ai_panel.pi_report = Some(crate::ai_panel::pi_ask::PiReportView {
                    summary: report.summary,
                    items: report.items,
                    workspace_label: report.workspace_label,
                });
                doc.ai_review_baseline = None;
                doc.ai_panel.finish(TurnState::Reported { fixes: 0 });
                self.status = format!(
                    "{prefix}Pi 交回 {items} 条侧栏清单，没有替换正文。工作区已保留在本机配置目录。"
                );
            }
            PiOutcome::Document(delivery) => {
                if let Err(error) = crate::pi_agent::verify_submission(
                    &delivery.path,
                    delivery.bytes,
                    &delivery.sha256,
                    &delivery.content,
                ) {
                    doc.ai_review_baseline = None;
                    doc.ai_panel.finish(TurnState::Failed(error.clone()));
                    self.status = format!("{prefix}Pi 智能写作交付检查未通过：{error}");
                    return;
                }
                // 轮次与运行引用先定位：写入、保留、被拒三条路都要用到；
                // 卡片已经不在了就不碰正文，成品留在工作区里。
                let located = doc
                    .ai_panel
                    .running_turn_mut()
                    .map(|turn| (turn.id, turn.pi_run.clone()));
                let Some((turn_id, reference)) = located else {
                    doc.ai_review_baseline = None;
                    self.status =
                        format!("{prefix}Pi 交回了成品，但这一轮已经不在了，未写入正文。");
                    return;
                };
                // 新交付只检查技术与权限保护，历史回执结构继续兼容。
                let mut report = delivery_check(&self.config, &delivery, &delivery.content);
                if let Some(reason) = report.blocking_reason() {
                    self.park_rejected_delivery(
                        index,
                        prefix,
                        turn_id,
                        &delivery.content,
                        reason,
                        reference.as_ref(),
                    );
                    return;
                }
                // 规整（清理、文种、validator、版式估算）之后才真正写进正文：对规整后的
                // 稿件再过一遍同样的检查，选区与公文引用都要按实际写入的文本成立。
                let result = crate::draft_page::reviewed_draft(
                    &doc.draft,
                    &self.config,
                    &delivery.content,
                    false,
                );
                // 规整全篇生效，但锁定选区只许改选区内：把选区外的规整改动还原成
                // 模型提交的原文，免得「忠实复现选区外原文」反被第二道检查误拒。
                let markdown = crate::pi_agent::restore_outside_selection(
                    &delivery.content,
                    &result.markdown,
                    delivery.selection,
                );
                report.absorb(delivery_check(&self.config, &delivery, &markdown));
                if let Some(reason) = report.blocking_reason() {
                    self.park_rejected_delivery(
                        index,
                        prefix,
                        turn_id,
                        &markdown,
                        reason,
                        reference.as_ref(),
                    );
                    return;
                }
                let advisories = report.advisories.clone();
                let provenance = format!(
                    "Pi Durable 智能写作；{}；提交文件 {}；工作区保留在本机配置目录",
                    delivery.workspace_label, delivery.path
                );
                let elapsed = delivery.elapsed;
                let Some(turn_id) = doc.ai_panel.running_turn_mut().map(|turn| {
                    turn.provenance = provenance;
                    turn.elapsed = Some(elapsed);
                    if let Some(run) = turn.pi_run.as_mut() {
                        run.status = crate::pi_agent::PiRunStatus::Delivered;
                        run.sources = delivery.sources_sha256.clone();
                        run.updated_at = chrono::Local::now().to_rfc3339();
                    }
                    if delivery.recovered {
                        turn.notes.push(
                            "这是从上次运行已经落盘的成品回执恢复的，没有重新生成正文。".into(),
                        );
                    }
                    for note in &delivery.confirmed {
                        turn.notes.push(format!("已确认：{note}"));
                    }
                    if !delivery.summary.trim().is_empty() {
                        turn.notes
                            .push(format!("提交说明：{}", delivery.summary.trim()));
                    }
                    turn.id
                }) else {
                    doc.ai_review_baseline = None;
                    self.status =
                        format!("{prefix}Pi 交回了成品，但这一轮已经不在了，未写入正文。");
                    return;
                };
                // 运行期间正文改过、稿件被发布 / 归档：正文与 AI 成品都不丢，
                // 成品留在轮次里等用户显式取回，或按当前正文重做。
                let conflict = doc.generated_markdown != delivery.before;
                let run_id = doc
                    .ai_panel
                    .turns
                    .iter()
                    .find(|turn| turn.id == turn_id)
                    .and_then(|turn| turn.pi_run.as_ref())
                    .map(|run| run.run_id.clone())
                    .unwrap_or_default();
                let pending = crate::ai_panel::pi_write::PiPendingDelivery {
                    run_id,
                    markdown: markdown.clone(),
                    advisories: advisories.clone(),
                };
                if let Some(turn) = doc.ai_panel.turn_mut(turn_id) {
                    turn.pi_pending = Some(pending.clone());
                }
                doc.ai_review_baseline = None;
                if doc.read_only() {
                    let reason =
                        "这篇稿件已发布或归档，只读，未写入正文；成品留在这一轮里。".to_string();
                    doc.ai_panel
                        .park_pi_delivery(turn_id, pending, false, reason.clone());
                    self.status = format!("{prefix}{reason}");
                    return;
                }
                if conflict {
                    let reason = "运行期间正文改过，未自动覆盖：AI 成品留在这一轮里，\
                                  可点「取回 AI 成品」（写入后可撤销），或按当前正文重做。"
                        .to_string();
                    doc.ai_panel
                        .park_pi_delivery(turn_id, pending, true, reason.clone());
                    self.status = format!("{prefix}{reason}");
                    return;
                }
                match crate::ai_panel::pi_write::commit(
                    doc,
                    self.manuscript_store.as_mut(),
                    ctx,
                    turn_id,
                    markdown,
                    advisories.clone(),
                ) {
                    Ok(()) => {
                        // 校对与预览按新正文重算；事实提示挂在 warnings 与卡片上，不挡写入。
                        self.draft_page_at(index).revalidate();
                        let doc = &mut self.docs[index];
                        for advisory in &advisories {
                            doc.warnings
                                .push(ReviewNote::from(format!("事实提示：{advisory}")));
                        }
                        self.status = if advisories.is_empty() {
                            format!("{prefix}Pi 已把成品直接写进正文，可撤销。")
                        } else {
                            format!(
                                "{prefix}Pi 已把成品直接写进正文（可撤销）；{} 条事实提示待核对。",
                                advisories.len()
                            )
                        };
                    }
                    Err(reason) => {
                        // `commit` 已把卡片退回「成品已保留」；正文没有被改。
                        self.status = format!("{prefix}Pi 成品没有写进正文：{reason}");
                    }
                }
            }
        }
    }
}

impl GongwenApp {
    /// 交付检查拦下：正文不动，但成品不丢——留在轮次里等用户显式「取回 AI 成品」
    /// （写入同样记撤销点、可撤销）或丢弃。运行记录标成被拒，「接着写」不再重放
    /// 同一份被拒回执；想再写只能从头开一轮。与现稿逐字相同的成品没有取回价值。
    fn park_rejected_delivery(
        &mut self,
        index: usize,
        prefix: &str,
        turn_id: u64,
        markdown: &str,
        reason: String,
        reference: Option<&crate::pi_agent::PiRunRef>,
    ) {
        let doc = &mut self.docs[index];
        doc.ai_review_baseline = None;
        let mut note = reason;
        if let Some(run) = reference
            && let Err(error) = crate::pi_agent::reject_run(run)
        {
            note = format!("{note}（运行记录没能标成被拒：{error}）");
        }
        if markdown == doc.generated_markdown {
            doc.ai_panel.finish(TurnState::Failed(note.clone()));
            self.status = format!("{prefix}Pi 智能写作交付检查未通过，未写入正文：{note}");
            return;
        }
        let pending = crate::ai_panel::pi_write::PiPendingDelivery {
            run_id: reference.map(|run| run.run_id.clone()).unwrap_or_default(),
            markdown: markdown.to_string(),
            advisories: Vec::new(),
        };
        let parked = doc
            .ai_panel
            .park_pi_delivery(turn_id, pending, false, note.clone());
        self.status = if parked {
            format!(
                "{prefix}Pi 成品未通过交付检查，未写入正文：{note}；成品留在这一轮里，\
                 可检查后显式取回（写入可撤销）或丢弃。"
            )
        } else {
            format!("{prefix}Pi 成品未通过交付检查，未写入正文：{note}")
        };
    }
}

#[cfg(test)]
mod pi_job_tests {
    use super::*;
    use crate::app::DocJob;
    use crate::app::management_samples;
    use crate::pi_agent::{PiDelivery, PiReport, PiReportItem, PiSelection};

    fn delivery(before: &str, content: &str) -> PiDelivery {
        crate::pi_agent::PiDelivery {
            before: before.into(),
            base: before.into(),
            request: "改一下正文。".into(),
            content: content.into(),
            path: "work/final.md".into(),
            summary: "测试".into(),
            sha256: crate::pi_agent::sha256_hex(content),
            bytes: content.len(),
            selection: None,
            fact_sources: Vec::new(),
            confirmed: Vec::new(),
            workspace_label: "pi/workspaces/test".into(),
            elapsed: std::time::Duration::from_secs(1),
            recovered: false,
            sources_sha256: None,
        }
    }

    fn report() -> PiReport {
        PiReport {
            summary: "审完".into(),
            items: vec![PiReportItem {
                title: "称谓".into(),
                detail: "第二条".into(),
                severity: "建议".into(),
            }],
            confirmed: Vec::new(),
            workspace_label: "pi/workspaces/test".into(),
            elapsed: std::time::Duration::from_secs(1),
            recovered: false,
            sources_sha256: None,
        }
    }

    /// 停止或新任务之后，迟到的 Pi 结果不能覆盖当前稿件；当前结果直接写进正文。
    #[test]
    fn a_style_file_can_never_be_written_into_the_manuscript_body() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文不能被风格JSON替换。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        app.docs[0]
            .ai_panel
            .push_turn("风格学习".into(), "学风格".into(), Vec::new(), None);
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Style(delivery(
                before,
                r#"{"name":"风格","description":"写法"}"#,
            ))))),
        );
        assert_eq!(app.docs[0].generated_markdown, before);
        assert!(app.docs[0].ai_proposal.is_none());
        assert!(
            app.docs[0]
                .ai_panel
                .turns
                .last()
                .unwrap()
                .pi_pending
                .is_none()
        );
    }

    #[test]
    fn a_late_pi_delivery_cannot_replace_a_newer_job() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 9;
        app.docs[0].busy = true;
        let turn = app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "改一下正文。".into(),
            vec!["Pi 文件工作区".into()],
            None,
        );
        app.docs[0].ai_panel.turn_mut(turn).unwrap().state = TurnState::Streaming;
        let key = app.docs[0].key;

        app.apply_doc_job(
            key,
            8,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n迟到的改动。\n",
            ))))),
        );
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);

        app.apply_doc_job(
            key,
            9,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n改后的正文。\n",
            ))))),
        );
        assert!(app.docs[0].ai_proposal.is_none(), "不再装提案");
        assert_eq!(app.docs[0].generated_markdown, "# 通知\n\n改后的正文。\n");
        assert!(!app.docs[0].busy);
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert_eq!(turn.state, TurnState::Accepted);
        let write = turn.pi_write.as_ref().expect("要留下撤销记录");
        assert_eq!(write.before, before);
        assert_eq!(write.after, "# 通知\n\n改后的正文。\n");
    }

    /// 技术 / 权限问题仍然拦下：正文不动；成品留在这一轮里，可显式取回或丢弃。
    #[test]
    fn a_technical_blocker_keeps_the_document_untouched() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "改一下正文。".into(),
            Vec::new(),
            None,
        );
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n见 work/终稿.md。\n",
            ))))),
        );
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert!(matches!(&turn.state, TurnState::Failed(reason) if reason.contains("工作区路径")));
        assert!(
            turn.pi_pending.is_some(),
            "被拦的成品也要留在轮次里，用户检查后还能显式取回"
        );
    }

    /// 选区任务 × 规整：模型忠实复现选区外原文（含无句号的列表项），规整若给
    /// 选区外补标点，写入时只保留选区内的规整，选区外逐字不动——不再整轮误拒。
    #[test]
    fn normalization_never_changes_text_outside_the_selection() {
        let mut app = management_samples::fixture();
        app.docs[0].draft.kind = crate::models::TemplateKind::PlainDocument;
        let before = "# 通知\n\n1. 加强领导\n\n第二段要改。\n\n第三段。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 5;
        app.docs[0].busy = true;
        app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "只改第二段。".into(),
            Vec::new(),
            None,
        );
        let start = before.find("第二段要改。").unwrap();
        let mut rejected = delivery(before, &before.replace("第二段要改。", "第二段已改。"));
        rejected.selection = Some(PiSelection {
            start,
            end: start + "第二段要改。".len(),
        });
        rejected.request = "只改第二段".into();
        // 先确认规整确实会动选区外的列表标点——否则这条回归形同虚设。
        let normalized = crate::draft_page::reviewed_draft(
            &app.docs[0].draft,
            &app.config,
            &rejected.content,
            false,
        );
        assert!(
            normalized.markdown.contains("1. 加强领导。"),
            "规整应给无句号的列表项补句号：{}",
            normalized.markdown
        );
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            5,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(rejected)))),
        );
        assert_eq!(
            app.docs[0].generated_markdown, "# 通知\n\n1. 加强领导\n\n第二段已改。\n\n第三段。\n",
            "选区外必须逐字保持原样（含规整想补句号的列表项）"
        );
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert_eq!(turn.state, TurnState::Accepted);
        assert!(
            turn.pi_write
                .as_ref()
                .is_some_and(|write| write.before == before),
            "照常留下撤销记录"
        );
    }

    /// 新日期、数字不产生事实核查卡片或待处理工作。
    #[test]
    fn pi_writes_do_not_create_fact_review_tasks() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "改一下正文。".into(),
            Vec::new(),
            None,
        );
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n新增2026年10月8日，共12项。\n",
            ))))),
        );
        assert_eq!(
            app.docs[0].generated_markdown, "# 通知\n\n新增2026年10月8日，共12项。\n",
            "事实疑点不能阻止写入"
        );
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert_eq!(turn.state, TurnState::Accepted);
        assert!(
            !turn
                .notes
                .iter()
                .any(|note| note.contains("未在本轮") || note.contains("事实提示"))
        );
        assert!(
            !app.docs[0]
                .warnings
                .iter()
                .any(|note| note.message.contains("事实提示"))
        );
    }

    /// 运行期间改过正文：不自动覆盖，成品留在轮次里；显式取回后仍可撤销。
    #[test]
    fn edits_during_the_run_are_kept_until_the_user_takes_the_draft_back() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "改一下正文。".into(),
            Vec::new(),
            None,
        );
        let key = app.docs[0].key;
        // 跑着的时候用户自己加了一句。
        let edited = "# 通知\n\n原文。用户加的一句。\n";
        app.docs[0].generated_markdown = edited.into();
        app.apply_doc_job(
            key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n改后的正文。\n",
            ))))),
        );
        assert_eq!(app.docs[0].generated_markdown, edited, "不能盖掉用户的改动");
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert_eq!(turn.state, TurnState::Conflict);
        assert_eq!(
            turn.pi_pending.as_ref().map(|p| p.markdown.as_str()),
            Some("# 通知\n\n改后的正文。\n"),
            "AI 成品也要留着"
        );
        let turn_id = turn.id;
        // 显式取回：写进正文，撤销能退回用户当时的版本。
        assert!(take_back(&mut app, turn_id), "取回要成功");
        assert_eq!(app.docs[0].generated_markdown, "# 通知\n\n改后的正文。\n");
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert_eq!(turn.state, TurnState::Accepted);
        assert_eq!(
            turn.pi_write.as_ref().map(|w| w.before.as_str()),
            Some(edited),
            "撤销要退回用户改过的那一版"
        );
    }

    /// 同一轮回执只能应用一次：已写过就不再重复盖正文。
    #[test]
    fn the_same_run_cannot_be_written_twice() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        let turn = waiting_turn(&mut app, 3);
        app.docs[0].ai_panel.turn_mut(turn).unwrap().pi_run = Some(run_ref(before));
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n改后的正文。\n",
            ))))),
        );
        assert_eq!(app.docs[0].generated_markdown, "# 通知\n\n改后的正文。\n");
        // 同一份运行标识再交一次（例如重启后重放回执）：正文不再变。
        app.docs[0].job_seq = 4;
        app.docs[0].busy = true;
        let again = app.docs[0].ai_panel.push_turn(
            "智能写作（Pi · 恢复）".into(),
            "接着改。".into(),
            Vec::new(),
            None,
        );
        let mut reference = run_ref("# 通知\n\n改后的正文。\n");
        reference.run_id = "run-1".into();
        app.docs[0].ai_panel.turn_mut(again).unwrap().pi_run = Some(reference);
        app.apply_doc_job(
            app.docs[0].key,
            4,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                "# 通知\n\n改后的正文。\n",
                "# 通知\n\n另一版。\n",
            ))))),
        );
        assert_eq!(
            app.docs[0].generated_markdown, "# 通知\n\n改后的正文。\n",
            "同一轮不能重复应用"
        );
    }

    /// 有库稿件存不下撤销记录就不写正文：成品留在轮次里，可以稍后重试。
    #[test]
    fn a_library_document_keeps_the_draft_when_the_undo_record_cannot_be_saved() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].manuscript_id = Some(7);
        app.docs[0].ai_panel.session.loaded_for = None;
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "改一下正文。".into(),
            Vec::new(),
            None,
        );
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n改后的正文。\n",
            ))))),
        );
        assert_eq!(app.docs[0].generated_markdown, before, "没落盘就不换正文");
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert!(matches!(turn.state, TurnState::Failed(_)));
        assert!(turn.pi_pending.is_some(), "成品要留着");
        assert!(turn.pi_write.is_none());
    }

    /// 卡片撤销：整次退回原文；写入后又手工改过就拒绝，不冲掉新编辑。
    #[test]
    fn a_write_can_be_undone_by_the_card_but_not_over_later_edits() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        let after = "# 通知\n\n改后的正文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        let turn_id = waiting_turn(&mut app, 3);
        app.apply_doc_job(
            app.docs[0].key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(before, after))))),
        );
        assert_eq!(app.docs[0].generated_markdown, after);
        assert!(
            crate::ai_panel::pi_write::undo(&mut app.docs[0], None, None, turn_id).is_ok(),
            "卡片撤销要能整次退回"
        );
        assert_eq!(app.docs[0].generated_markdown, before);
        let turn = app.docs[0]
            .ai_panel
            .turns
            .iter()
            .find(|turn| turn.id == turn_id)
            .unwrap();
        assert_eq!(turn.state, TurnState::Accepted, "卡片留着，标成已撤销");
        assert!(turn.pi_write.as_ref().is_some_and(|write| write.undone));
        // 再撤一次：没有可撤的了。
        assert!(crate::ai_panel::pi_write::undo(&mut app.docs[0], None, None, turn_id).is_err());
        assert_eq!(app.docs[0].generated_markdown, before);

        // 写入后用户又在正文里加了一句：卡片撤销不得整篇盖回去。
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 4;
        app.docs[0].busy = true;
        let second = waiting_turn(&mut app, 4);
        app.apply_doc_job(
            app.docs[0].key,
            4,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(before, after))))),
        );
        let edited = format!("{after}用户后来加的一句。\n");
        app.docs[0].generated_markdown = edited.clone();
        let error = crate::ai_panel::pi_write::undo(&mut app.docs[0], None, None, second)
            .expect_err("写入后改过就要拒绝");
        assert!(error.contains("Ctrl+Z"), "{error}");
        assert_eq!(app.docs[0].generated_markdown, edited);
    }

    /// 已入库稿件：撤销记录随会话落盘，重启后仍能找到修改前的原文。
    #[test]
    fn a_written_library_draft_keeps_its_before_text_across_restart() {
        let mut app = management_samples::fixture();
        let id = app
            .manuscript_store
            .as_mut()
            .unwrap()
            .create(
                &crate::manuscript::NewManuscript {
                    content_markdown: "".into(),
                    status: crate::models::ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let before = "# 通知\n\n原文。\n";
        let after = "# 通知\n\n改后的正文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].manuscript_id = Some(id);
        app.docs[0].ai_panel.session.loaded_for = Some(id);
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        let turn_id = waiting_turn(&mut app, 3);
        app.apply_doc_job(
            app.docs[0].key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(before, after))))),
        );
        assert_eq!(app.docs[0].generated_markdown, after);
        assert_eq!(
            app.docs[0]
                .ai_panel
                .turns
                .iter()
                .find(|turn| turn.id == turn_id)
                .and_then(|turn| turn.pi_write.as_ref())
                .map(|write| write.before.as_str()),
            Some(before)
        );
        // 库里的会话已经带上撤销记录，换一个面板读回也一样。
        let store = app.manuscript_store.as_mut().unwrap();
        let records = store.list_ai_sessions(id).unwrap();
        assert_eq!(records.len(), 1, "写入要把会话写进库");
        let rows = store.load_ai_session_turns(&records[0].id).unwrap();
        let mut panel = crate::ai_panel::AiPanel::default();
        let (_proposal, problems) = panel.load_session(id, &records[0], rows, after);
        assert!(problems.is_empty(), "{problems:?}");
        let restored = panel.turns.last().unwrap();
        assert_eq!(restored.state, TurnState::Accepted);
        assert_eq!(
            restored
                .pi_write
                .as_ref()
                .map(|write| write.before.as_str()),
            Some(before)
        );
    }

    /// 正文写入不碰表单：标题与授权要素全部保持原样。
    #[test]
    fn a_pi_write_only_touches_the_markdown() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].draft.title_hint = "原题名".into();
        app.docs[0].draft.profile.document_number = "12".into();
        let draft = app.docs[0].draft.clone();
        app.docs[0].job_seq = 3;
        app.docs[0].busy = true;
        app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "改一下正文。".into(),
            Vec::new(),
            None,
        );
        app.apply_doc_job(
            app.docs[0].key,
            3,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 另一个题名\n\n改后的正文。\n",
            ))))),
        );
        assert_eq!(app.docs[0].draft.title_hint, draft.title_hint);
        assert_eq!(
            app.docs[0].draft.profile.document_number,
            draft.profile.document_number
        );
    }

    /// `finish_report` 只进侧栏：既没有提案，也不改正文。
    #[test]
    fn a_pi_report_never_installs_a_proposal() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 4;
        app.docs[0].busy = true;
        app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "帮我审一下。".into(),
            Vec::new(),
            None,
        );
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            4,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Report(report())))),
        );
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);
        assert!(app.docs[0].ai_panel.pi_report.is_some());
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert!(!turn.is_workspace);
        assert!(turn.content.contains("审完"));
        assert!(turn.content.contains("称谓：第二条"));
        assert!(matches!(
            app.docs[0].ai_panel.turns.last().map(|turn| &turn.state),
            Some(TurnState::Reported { .. })
        ));
    }

    /// 选区外改动被程序拒绝；失败卡片留着原因，不装提案。
    #[test]
    fn a_selection_violation_is_rejected() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n第一段原样。\n\n第二段要改。\n\n第三段原样。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].job_seq = 5;
        app.docs[0].busy = true;
        app.docs[0].ai_panel.push_turn(
            "智能写作（Pi 试用）".into(),
            "只改第二段。".into(),
            Vec::new(),
            None,
        );
        let start = before.find("第二段").unwrap();
        let mut delivery = delivery(
            before,
            &before.replace("第三段原样。", "第三段被顺手改了。"),
        );
        delivery.selection = Some(PiSelection {
            start,
            end: start + "第二段要改。".len(),
        });
        delivery.request = "只改第二段".into();
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            5,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery)))),
        );
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);
        let turn = app.docs[0].ai_panel.turns.last().unwrap();
        assert!(
            matches!(&turn.state, TurnState::Failed(reason) if reason.contains("选区以外")),
            "{:?}",
            turn.state
        );
        assert!(
            turn.pi_pending.is_some(),
            "选区外改动被拦时成品也留着，用户确认后仍可显式取回"
        );
    }

    fn run_ref(before: &str) -> crate::pi_agent::PiRunRef {
        crate::pi_agent::PiRunRef {
            run_id: "run-1".into(),
            request_id: "req-1".into(),
            workspace: "pi/workspaces/doc-1/run-1".into(),
            baseline: crate::pi_agent::sha256_hex(before),
            status: crate::pi_agent::PiRunStatus::Running,
            model_provider: "p1".into(),
            model: "pi".into(),
            sources: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn proposal(before: &str) -> crate::draft_page::AiProposal {
        crate::draft_page::AiProposal {
            before: before.into(),
            result: crate::models::GeneratedDraft {
                markdown: "# 通知\n\n候选。\n".into(),
                title: String::new(),
                warnings: Vec::new(),
                proof_warnings: Vec::new(),
                proof_measured: false,
                files: Vec::new(),
            },
            label: "旧提案".into(),
            fact_changes: Vec::new(),
            fact_changes_confirmed: false,
            confirmed_facts: Vec::new(),
            excluded: Default::default(),
            view: Default::default(),
            open: true,
            locate: None,
        }
    }

    /// 测试用：走与侧栏「取回 AI 成品」同一条提交路径（不依赖绘制）。
    fn take_back(app: &mut GongwenApp, turn_id: u64) -> bool {
        let index = app.active_doc;
        let Some(pending) = app.docs[index]
            .ai_panel
            .turns
            .iter()
            .find(|turn| turn.id == turn_id)
            .and_then(|turn| turn.pi_pending.clone())
        else {
            return false;
        };
        let committed = crate::ai_panel::pi_write::commit(
            &mut app.docs[index],
            app.manuscript_store.as_mut(),
            None,
            turn_id,
            pending.markdown,
            pending.advisories.clone(),
        );
        if committed.is_ok() {
            app.draft_page_at(index).revalidate();
        }
        committed.is_ok()
    }

    fn waiting_turn(app: &mut GongwenApp, seq: u64) -> u64 {
        let turn = app.docs[0].ai_panel.push_turn(
            "智能写作（Pi）".into(),
            "改一下正文。".into(),
            vec!["Pi 文件工作区".into()],
            None,
        );
        let item = app.docs[0].ai_panel.turn_mut(turn).unwrap();
        item.state = TurnState::Waiting;
        item.usage_job_seq = Some(seq);
        turn
    }

    #[test]
    fn a_real_session_save_failure_preserves_a_retryable_delivery() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        // 内存库中不存在此稿件，触发实际外键写入失败，而非缺少 store 的前置拒绝。
        app.docs[0].manuscript_id = Some(99999);
        app.docs[0].ai_panel.adopt_session(99999);
        app.docs[0].job_seq = 1;
        app.docs[0].busy = true;
        let turn_id = waiting_turn(&mut app, 1);
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            1,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n成品。\n",
            ))))),
        );
        assert_eq!(app.docs[0].generated_markdown, before);
        let turn = app.docs[0].ai_panel.turn_mut(turn_id).unwrap();
        assert!(matches!(turn.state, TurnState::Failed(_)));
        assert!(turn.pi_write.is_none());
        assert_eq!(
            turn.pi_pending.as_ref().unwrap().markdown,
            "# 通知\n\n成品。\n"
        );
        app.docs[0].manuscript_id = None;
        assert!(
            take_back(&mut app, turn_id),
            "保存问题解决后应能取回同一份成品"
        );
    }

    #[test]
    fn taking_back_a_delivery_never_writes_a_read_only_document() {
        for status in [
            crate::models::ManuscriptStatus::Published,
            crate::models::ManuscriptStatus::Archived,
        ] {
            let mut app = management_samples::fixture();
            app.docs[0].generated_markdown = "原文".into();
            app.docs[0].manuscript_id = Some(1);
            app.docs[0].record_status = status;
            let turn_id = waiting_turn(&mut app, 1);
            app.docs[0].ai_panel.park_pi_delivery(
                turn_id,
                crate::ai_panel::pi_write::PiPendingDelivery {
                    run_id: String::new(),
                    markdown: "成品".into(),
                    advisories: Vec::new(),
                },
                false,
                "只读".into(),
            );
            assert!(!take_back(&mut app, turn_id));
            assert_eq!(app.docs[0].generated_markdown, "原文");
            assert!(
                app.docs[0]
                    .ai_panel
                    .turn_mut(turn_id)
                    .unwrap()
                    .pi_pending
                    .is_some()
            );
        }
    }

    #[test]
    fn a_manual_list_punctuation_edit_is_a_conflict_too() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n1. 原文。\n";
        let edited = "# 通知\n\n1、原文。\n";
        app.docs[0].generated_markdown = edited.into();
        app.docs[0].job_seq = 1;
        app.docs[0].busy = true;
        waiting_turn(&mut app, 1);
        let key = app.docs[0].key;
        app.apply_doc_job(
            key,
            1,
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n改后的正文。\n",
            ))))),
        );
        assert_eq!(app.docs[0].generated_markdown, edited);
        assert!(matches!(
            app.docs[0].ai_panel.turns.last().unwrap().state,
            TurnState::Conflict
        ));
    }

    /// 准备完成事件绑定运行引用，但不提前清 busy；host 还没开始跑。
    #[test]
    fn prepared_event_binds_the_run_reference_and_keeps_the_document_busy() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].ai_proposal = Some(proposal(before));
        app.docs[0].job_seq = 5;
        app.docs[0].busy = true;
        let turn = waiting_turn(&mut app, 5);
        let key = app.docs[0].key;
        let (ack, rx) = crate::ai_panel::pi_prepare::PiPrepareAck::channel();
        let bridge = std::sync::Arc::new(crate::pi_agent::PiBridge::default());
        app.apply_doc_job(
            key,
            5,
            DocJob::PiPrepared {
                turn_id: turn,
                reference: run_ref(before),
                dev_runtime: true,
                bridge: bridge.clone(),
                kind: app.docs[0].draft.kind.label().into(),
                ack,
            },
        );
        assert!(rx.try_recv().expect("界面必须确认").is_ok());
        let doc = &app.docs[0];
        let item = doc
            .ai_panel
            .turns
            .iter()
            .find(|item| item.id == turn)
            .unwrap();
        assert_eq!(
            item.pi_run.as_ref().map(|run| run.run_id.as_str()),
            Some("run-1")
        );
        assert!(item.context.iter().any(|line| line.contains("开发运行时")));
        assert!(doc.ai_proposal.is_none(), "成功绑定后才清旧提案");
        assert!(
            std::sync::Arc::ptr_eq(doc.ai_panel.pi_bridge.as_ref().unwrap(), &bridge),
            "问答必须路由到后台 host 的同一条通道"
        );
        assert!(doc.busy, "准备完成不能提前释放 busy");
    }

    /// 过期事件不绑定、不确认；ack 被丢弃，后台的有界等待会结束。
    #[test]
    fn a_stale_prepared_event_is_dropped_without_binding() {
        let mut app = management_samples::fixture();
        app.docs[0].job_seq = 6;
        app.docs[0].busy = true;
        let turn = waiting_turn(&mut app, 6);
        let key = app.docs[0].key;
        let (ack, rx) = crate::ai_panel::pi_prepare::PiPrepareAck::channel();
        app.apply_doc_job(
            key,
            5,
            DocJob::PiPrepared {
                turn_id: turn,
                reference: run_ref("# 通知\n"),
                dev_runtime: false,
                bridge: std::sync::Arc::new(crate::pi_agent::PiBridge::default()),
                kind: app.docs[0].draft.kind.label().into(),
                ack,
            },
        );
        assert!(rx.try_recv().is_err(), "过期事件不能回 ack");
        let item = app.docs[0]
            .ai_panel
            .turns
            .iter()
            .find(|item| item.id == turn)
            .unwrap();
        assert!(item.pi_run.is_none());
    }

    /// 有库稿件保存不了运行引用就不启动 host：ack 回错、撤掉恢复入口，
    /// 随后 PiDone(Err) 正常收尾 busy。
    #[test]
    fn a_library_document_must_save_the_run_reference_before_starting() {
        let mut app = management_samples::fixture();
        app.docs[0].generated_markdown = "# 通知\n\n原文。\n".into();
        app.docs[0].ai_proposal = Some(proposal("# 通知\n\n原文。\n"));
        app.docs[0].manuscript_id = Some(7);
        app.docs[0].ai_panel.session.loaded_for = None;
        app.docs[0].job_seq = 5;
        app.docs[0].busy = true;
        let turn = waiting_turn(&mut app, 5);
        let key = app.docs[0].key;
        let (ack, rx) = crate::ai_panel::pi_prepare::PiPrepareAck::channel();
        app.apply_doc_job(
            key,
            5,
            DocJob::PiPrepared {
                turn_id: turn,
                reference: run_ref("# 通知\n\n原文。\n"),
                dev_runtime: false,
                bridge: std::sync::Arc::new(crate::pi_agent::PiBridge::default()),
                kind: app.docs[0].draft.kind.label().into(),
                ack,
            },
        );
        assert!(rx.try_recv().expect("必须回 ack").is_err());
        let item = app.docs[0]
            .ai_panel
            .turns
            .iter()
            .find(|item| item.id == turn)
            .unwrap();
        assert!(item.pi_run.is_none(), "没保存就不能给恢复入口");
        assert!(app.docs[0].ai_proposal.is_some(), "保存失败必须归还旧提案");
        app.apply_doc_job(key, 5, DocJob::PiDone(Box::new(Err("未启动 host".into()))));
        assert!(!app.docs[0].busy, "准备失败必须收尾 busy");
    }

    /// 准备失败不能顺手清掉已有候选提案：用户还得能继续审阅。
    #[test]
    fn a_failed_preparation_keeps_the_existing_proposal() {
        let mut app = management_samples::fixture();
        let before = "# 通知\n\n原文。\n";
        app.docs[0].generated_markdown = before.into();
        app.docs[0].ai_proposal = Some(proposal(before));
        app.docs[0].job_seq = 5;
        app.docs[0].busy = true;
        waiting_turn(&mut app, 5);
        let key = app.docs[0].key;
        app.apply_doc_job(key, 5, DocJob::PiDone(Box::new(Err("读不到引用".into()))));
        assert!(app.docs[0].ai_proposal.is_some(), "准备失败要保留旧提案");
        assert!(!app.docs[0].busy);
    }
}
