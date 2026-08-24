//! 公文办理流程的状态判定。
//!
//! 「智能体」在这里落成一台**确定性状态机**，而不是让大模型自由调工具。理由是
//! 公文场景要的东西通用 Agent 给不了：谁在第几步改了哪一句、依据哪条规则、
//! 用户是否确认过——全部可回放、可审计、可复现。模型一旦拿到流程控制权，
//! 这三样立刻都没了。
//!
//! 所以这里只做两件事：**算出每一步过没过**，以及**说清楚差什么**。
//!
//! 原计划里还有一半是「让大模型生成『当前稿件还差什么』的诊断摘要」，已取消。
//! 这些状态全都可计算——用模型去生成一份程序算得出的结论，只会更慢、更不准，
//! 而且它根本看不到 `validator` 的输出。能算的就不要猜。
//!
//! 另一个刻意的克制：**这是一张状态表，不是向导**。不强制按顺序推进，也不挡住
//! 任何操作。真实写公文并不线性——先润色后填要素是常事，强行排序只会让工具
//! 变难用。程序负责把「现在卡在哪」摆出来，走不走、先走哪步是人的事。

/// 办理流程的一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Draft,
    Elements,
    Proofread,
    ModelReview,
    OpenQuestions,
    Layout,
    Submit,
}

impl Stage {
    pub const ALL: [Self; 7] = [
        Self::Draft,
        Self::Elements,
        Self::Proofread,
        Self::ModelReview,
        Self::OpenQuestions,
        Self::Layout,
        Self::Submit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Draft => "拟稿",
            Self::Elements => "要素齐备",
            Self::Proofread => "文字校对",
            Self::ModelReview => "表达复核",
            Self::OpenQuestions => "存疑清零",
            Self::Layout => "版式验证",
            Self::Submit => "入库送审",
        }
    }

    /// 这一步为什么存在。界面上跟着标题显示——不说清楚，用户只会把它当装饰。
    pub fn purpose(self) -> &'static str {
        match self {
            Self::Draft => "有正文才谈得上后面各步",
            Self::Elements => "缺必填要素会被正式导出阻断",
            Self::Proofread => "必错级问题必须清零，疑似项由人判断",
            Self::ModelReview => "小模型逐句查语病、称谓与标点",
            Self::OpenQuestions => "正文里的「待核实」占位必须落实后才能送审",
            Self::Layout => "排版编译成功过一次，孤行与版式才算量准",
            Self::Submit => "存入稿件库并固化成版本，改动才有据可查",
        }
    }
}

/// 一步的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    /// 已达标。
    Passed,
    /// 还差事情。
    Todo,
    /// 这份稿子用不上这一步（如未配置复核模型）。
    NotApplicable,
}

/// 一步的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageStatus {
    pub stage: Stage,
    pub state: StageState,
    /// 差什么，或者为什么不适用。一句话，要能直接指向下一步动作。
    pub detail: String,
}

impl StageStatus {
    pub fn is_todo(&self) -> bool {
        self.state == StageState::Todo
    }
}

/// 判定所需的全部输入。
///
/// 单独一个快照结构而不是直接读 `DraftSession`：判定逻辑要能脱离整个应用单测，
/// 而 `DraftSession` 拖着 egui 的视图状态，测试里造不出来。
#[derive(Debug, Clone, Default)]
pub struct DocSnapshot {
    /// 正文是否为空。
    pub empty: bool,
    /// 会阻断正式导出的要素问题数。
    pub blocking_issues: usize,
    /// 待确认的必错级修订建议数。
    pub mustfix_pending: usize,
    /// 待确认的疑似/提示级建议数。只报数，不作为通过与否的判据——
    /// 疑似项本来就该由人决定留不留。
    pub advisory_pending: usize,
    /// 待确认的模型来源建议数。
    pub model_pending: usize,
    /// 这一篇跑过模型复核没有。
    pub model_review_ran: bool,
    /// 复核模型配没配。没配时表达复核这一步不适用。
    pub model_configured: bool,
    /// 正文里「【待核实」占位的个数。
    pub open_questions: usize,
    /// 导出并编译成功过。
    pub exported: bool,
    /// 最近一次导出/编译失败。
    pub export_failed: bool,
    /// 已存入稿件库。
    pub saved: bool,
    /// 已固化成版本。
    pub committed: bool,
    /// 存库之后又改过（脏）。
    pub dirty: bool,
}

/// 数一数正文里还剩几个「【待核实」占位。
///
/// 起草提示词要求模型缺依据时原位写「【待核实：缺什么】」而不是编造。那是好事，
/// 但**留在成稿里就是事故**——所以送审前必须清零，这一步就是那道闸。
pub fn count_open_questions(markdown: &str) -> usize {
    markdown.matches("【待核实").count()
}

/// 逐步判定。
pub fn evaluate(snapshot: &DocSnapshot) -> Vec<StageStatus> {
    Stage::ALL
        .into_iter()
        .map(|stage| {
            let (state, detail) = judge(stage, snapshot);
            StageStatus {
                stage,
                state,
                detail,
            }
        })
        .collect()
}

fn judge(stage: Stage, snap: &DocSnapshot) -> (StageState, String) {
    match stage {
        Stage::Draft => {
            if snap.empty {
                (
                    StageState::Todo,
                    "还没有正文：可用 AI 工作台起草，或直接粘贴".into(),
                )
            } else {
                (StageState::Passed, "已有正文".into())
            }
        }
        // 正文还没有时，后面几步一律「待办」而不是「不适用」：它们迟早都要做，
        // 标成不适用会让人以为可以跳过。
        Stage::Elements => {
            if snap.blocking_issues == 0 {
                (StageState::Passed, "必填要素齐备".into())
            } else {
                (
                    StageState::Todo,
                    format!("还有 {} 项会阻断正式导出", snap.blocking_issues),
                )
            }
        }
        Stage::Proofread => {
            if snap.mustfix_pending > 0 {
                return (
                    StageState::Todo,
                    format!(
                        "还有 {} 条必错建议待处理{}",
                        snap.mustfix_pending,
                        advisory_tail(snap.advisory_pending)
                    ),
                );
            }
            (
                StageState::Passed,
                format!("必错已清零{}", advisory_tail(snap.advisory_pending)),
            )
        }
        Stage::ModelReview => {
            if !snap.model_configured {
                return (StageState::NotApplicable, "未配置复核模型，本步跳过".into());
            }
            if !snap.model_review_ran {
                return (StageState::Todo, "还没有跑过文字复核".into());
            }
            if snap.model_pending > 0 {
                return (
                    StageState::Todo,
                    format!("复核给出的 {} 条建议还没逐条处理", snap.model_pending),
                );
            }
            (StageState::Passed, "复核已跑过，建议均已处理".into())
        }
        Stage::OpenQuestions => {
            if snap.open_questions == 0 {
                (StageState::Passed, "正文没有待核实占位".into())
            } else {
                (
                    StageState::Todo,
                    format!(
                        "正文里还有 {} 处「【待核实…】」，送审前必须落实",
                        snap.open_questions
                    ),
                )
            }
        }
        Stage::Layout => {
            if snap.export_failed {
                return (StageState::Todo, "最近一次导出或编译失败，请先排除".into());
            }
            if snap.exported {
                (StageState::Passed, "已成功导出并编译".into())
            } else {
                (StageState::Todo, "还没有导出过，孤行与版式尚未实测".into())
            }
        }
        Stage::Submit => {
            if !snap.saved {
                return (StageState::Todo, "还没有存入稿件库".into());
            }
            if !snap.committed {
                return (StageState::Todo, "已存库，但还没有固化成版本".into());
            }
            if snap.dirty {
                return (
                    StageState::Todo,
                    "存库后又有改动，请重新保存或提交版本".into(),
                );
            }
            (StageState::Passed, "已入库并固化成版本".into())
        }
    }
}

fn advisory_tail(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!("；另有 {count} 条疑似项由你判断")
    }
}

/// 已达标的步数与计入统计的总步数（不适用的不计）。
pub fn progress(statuses: &[StageStatus]) -> (usize, usize) {
    let counted: Vec<_> = statuses
        .iter()
        .filter(|item| item.state != StageState::NotApplicable)
        .collect();
    let passed = counted
        .iter()
        .filter(|item| item.state == StageState::Passed)
        .count();
    (passed, counted.len())
}

/// 当前最该做的那一步。按流程顺序取第一个未达标的。
pub fn next_todo(statuses: &[StageStatus]) -> Option<&StageStatus> {
    statuses.iter().find(|item| item.is_todo())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> DocSnapshot {
        DocSnapshot {
            empty: false,
            model_configured: true,
            model_review_ran: true,
            exported: true,
            saved: true,
            committed: true,
            ..Default::default()
        }
    }

    fn state_of(statuses: &[StageStatus], stage: Stage) -> StageState {
        statuses
            .iter()
            .find(|item| item.stage == stage)
            .expect("每一步都要有判定")
            .state
    }

    #[test]
    fn a_finished_document_passes_every_stage() {
        let statuses = evaluate(&clean());
        assert!(
            statuses.iter().all(|item| item.state == StageState::Passed),
            "干净稿件应当全部通过：{statuses:?}"
        );
        assert_eq!(progress(&statuses), (7, 7));
        assert!(next_todo(&statuses).is_none());
    }

    #[test]
    fn an_empty_document_is_blocked_at_drafting() {
        let snapshot = DocSnapshot {
            empty: true,
            ..clean()
        };
        let statuses = evaluate(&snapshot);
        assert_eq!(state_of(&statuses, Stage::Draft), StageState::Todo);
        assert_eq!(
            next_todo(&statuses).map(|item| item.stage),
            Some(Stage::Draft),
            "空稿的第一件事就是拟稿"
        );
    }

    /// 未配置复核模型时这一步不适用，而且**不计入分母**——
    /// 否则进度永远到不了满格，用户会以为自己漏了什么。
    #[test]
    fn an_unconfigured_model_makes_review_not_applicable_and_uncounted() {
        let snapshot = DocSnapshot {
            model_configured: false,
            model_review_ran: false,
            ..clean()
        };
        let statuses = evaluate(&snapshot);
        assert_eq!(
            state_of(&statuses, Stage::ModelReview),
            StageState::NotApplicable
        );
        assert_eq!(progress(&statuses), (6, 6));
        assert!(next_todo(&statuses).is_none());
    }

    #[test]
    fn a_configured_but_unrun_review_is_a_todo() {
        let snapshot = DocSnapshot {
            model_review_ran: false,
            ..clean()
        };
        assert_eq!(
            state_of(&evaluate(&snapshot), Stage::ModelReview),
            StageState::Todo
        );
    }

    /// 疑似项只报数，不卡流程：它本来就该由人决定留不留，
    /// 拿它当通过判据等于逼人把每条都点掉。
    #[test]
    fn advisory_suggestions_are_reported_but_do_not_block() {
        let snapshot = DocSnapshot {
            advisory_pending: 4,
            ..clean()
        };
        let statuses = evaluate(&snapshot);
        let proofread = statuses
            .iter()
            .find(|item| item.stage == Stage::Proofread)
            .expect("有文字校对这一步");
        assert_eq!(proofread.state, StageState::Passed);
        assert!(
            proofread.detail.contains("4 条疑似"),
            "{}",
            proofread.detail
        );
    }

    #[test]
    fn pending_mustfix_blocks_proofreading() {
        let snapshot = DocSnapshot {
            mustfix_pending: 3,
            ..clean()
        };
        assert_eq!(
            state_of(&evaluate(&snapshot), Stage::Proofread),
            StageState::Todo
        );
    }

    #[test]
    fn open_question_placeholders_block_submission() {
        let snapshot = DocSnapshot {
            open_questions: 2,
            ..clean()
        };
        let statuses = evaluate(&snapshot);
        assert_eq!(state_of(&statuses, Stage::OpenQuestions), StageState::Todo);
    }

    #[test]
    fn open_questions_are_counted_from_the_placeholder_marker() {
        // 起草提示词要模型缺依据时写「【待核实：…】」而不是编造。
        // 那是好事，但留在成稿里就是事故。
        let markdown = "第一段【待核实：会议时间】。\n第二段【待核实：参会人员】。\n";
        assert_eq!(count_open_questions(markdown), 2);
        assert_eq!(count_open_questions("干净的正文。"), 0);
    }

    #[test]
    fn a_failed_export_is_reported_separately_from_never_exported() {
        let never = DocSnapshot {
            exported: false,
            ..clean()
        };
        let failed = DocSnapshot {
            export_failed: true,
            ..clean()
        };
        let never = evaluate(&never);
        let failed = evaluate(&failed);
        let pick = |set: &[StageStatus]| {
            set.iter()
                .find(|item| item.stage == Stage::Layout)
                .expect("有版式这一步")
                .detail
                .clone()
        };
        assert!(pick(&never).contains("还没有导出过"));
        assert!(pick(&failed).contains("失败"));
    }

    #[test]
    fn saving_without_committing_is_still_a_todo() {
        let snapshot = DocSnapshot {
            committed: false,
            ..clean()
        };
        let statuses = evaluate(&snapshot);
        assert_eq!(state_of(&statuses, Stage::Submit), StageState::Todo);
        assert!(
            statuses
                .iter()
                .find(|item| item.stage == Stage::Submit)
                .is_some_and(|item| item.detail.contains("还没有固化成版本"))
        );
    }

    #[test]
    fn edits_after_saving_reopen_submission() {
        let snapshot = DocSnapshot {
            dirty: true,
            ..clean()
        };
        assert_eq!(
            state_of(&evaluate(&snapshot), Stage::Submit),
            StageState::Todo
        );
    }

    /// 未达标的步按流程顺序取第一个，而不是取最严重的——
    /// 用户要的是「现在该干什么」，不是「哪里最糟」。
    #[test]
    fn the_next_todo_follows_process_order() {
        let snapshot = DocSnapshot {
            blocking_issues: 1,
            mustfix_pending: 5,
            open_questions: 3,
            ..clean()
        };
        assert_eq!(
            next_todo(&evaluate(&snapshot)).map(|item| item.stage),
            Some(Stage::Elements)
        );
    }
}
