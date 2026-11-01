//! Pi 智能写作结果交付与异步任务隔离测试。

use super::GongwenApp;
use crate::ai_panel::TurnState;
use crate::models::ReviewNote;
use crate::pi_agent::{DeliveryCheck, PiOutcome};

impl GongwenApp {
    /// Pi 本轮结束：成功才安装提案或侧栏清单，失败只留原因，绝不动正文。
    pub(super) fn apply_pi_done(
        &mut self,
        index: usize,
        prefix: &str,
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
            PiOutcome::Report(report) => {
                let notes = report.confirmed;
                if let Some(turn) = doc.ai_panel.running_turn_mut() {
                    turn.elapsed = Some(report.elapsed);
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
                let issues = crate::pi_agent::check_delivery(&DeliveryCheck {
                    before: &delivery.before,
                    base: &delivery.base,
                    request: &delivery.request,
                    content: &delivery.content,
                    selection: delivery.selection,
                    fact_sources: &delivery.fact_sources,
                    vocabulary: &self.config.vocabulary,
                });
                if !issues.is_empty() {
                    let reason = issues.join("；");
                    doc.ai_review_baseline = None;
                    doc.ai_panel.finish(TurnState::Failed(reason.clone()));
                    self.status =
                        format!("{prefix}Pi 智能写作交付检查未通过，未生成提案：{reason}");
                    return;
                }
                // 交付仍走 reviewed_draft（清理、文种规整、validator、版式估算）与本地复扫；
                // 硬闸门通过后才装提案，用户接受时还会再过一遍 accept_ai_proposal。
                let mut result = crate::draft_page::reviewed_draft(
                    &doc.draft,
                    &self.config,
                    &delivery.content,
                    false,
                );
                // 清理和文种规整也可能改变选区外文字，须对实际安装的稿件再过闸门。
                let issues = crate::pi_agent::check_delivery(&DeliveryCheck {
                    before: &delivery.before,
                    base: &delivery.base,
                    request: &delivery.request,
                    content: &result.markdown,
                    selection: delivery.selection,
                    fact_sources: &delivery.fact_sources,
                    vocabulary: &self.config.vocabulary,
                });
                if !issues.is_empty() {
                    let reason = issues.join("；");
                    doc.ai_review_baseline = None;
                    doc.ai_panel.finish(TurnState::Failed(reason.clone()));
                    self.status = format!("{prefix}Pi 文稿规整后未通过检查：{reason}");
                    return;
                }
                let mut notes = crate::proofread::Lexicon::resolved(&self.config.proofread)
                    .check(&result.markdown);
                notes.extend(crate::proofread_rules::check_with_config(
                    &doc.draft,
                    &result.markdown,
                    &self.config.proofread,
                ));
                result
                    .warnings
                    .extend(notes.into_iter().map(ReviewNote::from));
                let provenance = format!(
                    "Pi Durable 智能写作；{}；提交文件 {}；工作区保留在本机配置目录",
                    delivery.workspace_label, delivery.path
                );
                let elapsed = delivery.elapsed;
                let mut sources = delivery.fact_sources.clone();
                sources.push(delivery.request.clone());
                sources.sort();
                sources.dedup();
                let proposal_sources = crate::ai_panel::pi_job::PiProposalSources {
                    before: delivery.before.clone(),
                    candidate: result.markdown.clone(),
                    sources,
                };
                let summary = GongwenApp::install_ai_proposal(
                    doc,
                    delivery.before,
                    result,
                    "Pi 智能写作".into(),
                    &self.config.vocabulary,
                );
                doc.ai_panel.pi_sources = Some(proposal_sources);
                if let Some(turn) = doc.ai_panel.running_turn_mut() {
                    turn.provenance = provenance;
                    turn.elapsed = Some(elapsed);
                    for note in delivery.confirmed {
                        turn.notes.push(format!("已确认：{note}"));
                    }
                    if !delivery.summary.trim().is_empty() {
                        turn.notes
                            .push(format!("提交说明：{}", delivery.summary.trim()));
                    }
                }
                doc.ai_review_baseline = None;
                doc.ai_panel.finish(TurnState::Proposed(summary));
                if let Some(proposal) = doc.ai_proposal.as_mut() {
                    proposal.open = true;
                }
                self.status = format!(
                    "{prefix}Pi 智能写作已交出一份提案；接受前正文不变，请先核对关键事实变化。"
                );
            }
        }
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
        }
    }

    /// 停止或新任务之后，迟到的 Pi 结果不能覆盖当前稿件；当前结果只装提案，
    /// 不直接写 `generated_markdown`。
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
            DocJob::PiDone(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n迟到的改动。\n",
            )))),
        );
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);

        app.apply_doc_job(
            key,
            9,
            DocJob::PiDone(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n改后的正文。\n",
            )))),
        );
        let proposal = app.docs[0]
            .ai_proposal
            .as_ref()
            .expect("当前结果应装成提案");
        assert_eq!(proposal.before, before);
        assert_eq!(proposal.result.markdown, "# 通知\n\n改后的正文。\n");
        assert_eq!(app.docs[0].generated_markdown, before);
        assert!(!app.docs[0].busy);
        assert!(matches!(
            app.docs[0].ai_panel.turns.last().map(|turn| &turn.state),
            Some(TurnState::Proposed(_))
        ));
        let doc = &app.docs[0];
        let sources = doc.ai_panel.pi_sources.as_ref().unwrap();
        assert!(
            sources
                .for_document(doc)
                .iter()
                .any(|text| text == "改一下正文。")
        );
        assert!(
            !sources
                .for_document(doc)
                .iter()
                .any(|text| text.contains("改后的正文")),
            "候选稿不能自行变成事实依据"
        );
        app.docs[0].generated_markdown.push_str("用户新改动");
        let doc = &app.docs[0];
        assert!(
            doc.ai_panel
                .pi_sources
                .as_ref()
                .unwrap()
                .for_document(doc)
                .is_empty()
        );
    }

    /// 来源检查不过时只有失败卡片，没有提案。
    #[test]
    fn a_failed_pi_delivery_installs_no_proposal() {
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
            DocJob::PiDone(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n新增2026年10月8日，共12项。\n",
            )))),
        );
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);
        assert!(matches!(
            app.docs[0].ai_panel.turns.last().map(|turn| &turn.state),
            Some(TurnState::Failed(_))
        ));
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
        app.apply_doc_job(key, 4, DocJob::PiDone(Ok(PiOutcome::Report(report()))));
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
        app.apply_doc_job(key, 5, DocJob::PiDone(Ok(PiOutcome::Document(delivery))));
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);
        assert!(matches!(
            app.docs[0].ai_panel.turns.last().map(|turn| &turn.state),
            Some(TurnState::Failed(reason)) if reason.contains("选区以外")
        ));
    }
}
