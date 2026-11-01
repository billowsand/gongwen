//! Pi 智能写作结果交付与异步任务隔离测试。

use super::GongwenApp;
use crate::ai_panel::TurnState;
use crate::models::ReviewNote;

impl GongwenApp {
    /// Pi 智能写作结束：成功才安装提案，失败只留原因，绝不动正文。
    pub(super) fn apply_pi_done(
        &mut self,
        index: usize,
        prefix: &str,
        result: Result<crate::pi_agent::PiDelivery, String>,
    ) {
        let doc = &mut self.docs[index];
        let delivery = match result {
            Ok(delivery) => delivery,
            Err(error) => {
                doc.ai_review_baseline = None;
                doc.ai_panel.finish(TurnState::Failed(error.clone()));
                self.status = format!("{prefix}Pi 智能写作未完成：{error}");
                return;
            }
        };
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
        let issues = crate::pi_agent::check_delivery(
            &delivery.before,
            &delivery.request,
            &delivery.content,
            &self.config.vocabulary,
        );
        if !issues.is_empty() {
            let reason = issues.join("；");
            doc.ai_review_baseline = None;
            doc.ai_panel.finish(TurnState::Failed(reason.clone()));
            self.status = format!("{prefix}Pi 智能写作交付检查未通过，未生成提案：{reason}");
            return;
        }
        // 交付仍走 reviewed_draft（清理、文种规整、validator、版式估算）与本地复扫；
        // 硬闸门通过后才装提案，用户接受时还会再过一遍 accept_ai_proposal。
        let mut result =
            crate::draft_page::reviewed_draft(&doc.draft, &self.config, &delivery.content, false);
        let mut notes =
            crate::proofread::Lexicon::resolved(&self.config.proofread).check(&result.markdown);
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
        let summary = GongwenApp::install_ai_proposal(
            doc,
            delivery.before,
            result,
            "Pi 智能写作".into(),
            &self.config.vocabulary,
        );
        if let Some(turn) = doc.ai_panel.running_turn_mut() {
            turn.provenance = provenance;
            turn.elapsed = Some(elapsed);
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
        self.status =
            format!("{prefix}Pi 智能写作已交出一份提案；接受前正文不变，请先核对关键事实变化。");
    }
}

#[cfg(test)]
mod pi_job_tests {
    use super::*;
    use crate::app::DocJob;
    use crate::app::management_samples;

    fn delivery(before: &str, content: &str) -> crate::pi_agent::PiDelivery {
        crate::pi_agent::PiDelivery {
            before: before.into(),
            request: "改一下正文。".into(),
            content: content.into(),
            path: "work/final.md".into(),
            summary: "测试".into(),
            sha256: crate::pi_agent::sha256_hex(content),
            bytes: content.len(),
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
            DocJob::PiDone(Ok(delivery(before, "# 通知\n\n迟到的改动。\n"))),
        );
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);

        app.apply_doc_job(
            key,
            9,
            DocJob::PiDone(Ok(delivery(before, "# 通知\n\n改后的正文。\n"))),
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
            DocJob::PiDone(Ok(delivery(
                before,
                "# 通知\n\n新增2026年10月8日，共12项。\n",
            ))),
        );
        assert!(app.docs[0].ai_proposal.is_none());
        assert_eq!(app.docs[0].generated_markdown, before);
        assert!(matches!(
            app.docs[0].ai_panel.turns.last().map(|turn| &turn.state),
            Some(TurnState::Failed(_))
        ));
    }
}
