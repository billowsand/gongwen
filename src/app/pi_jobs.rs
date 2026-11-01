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
            DocJob::PiDone(Box::new(Ok(PiOutcome::Document(delivery(
                before,
                "# 通知\n\n新增2026年10月8日，共12项。\n",
            ))))),
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
        assert!(matches!(
            app.docs[0].ai_panel.turns.last().map(|turn| &turn.state),
            Some(TurnState::Failed(reason)) if reason.contains("选区以外")
        ));
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
