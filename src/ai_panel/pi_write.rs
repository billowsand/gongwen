//! Pi 成品直接写入正文的撤销记录与冲突产物。
//!
//! 产品修订（2026-11-02，见 `docs/pi-implementation/pi-direct-write-task.md`）：Pi 明确
//! 提交的有效 Markdown 由界面线程直接写进当前可编辑正文，普通流程不再弹人工校订窗、
//! 不要求勾选事实、不要求接受提案。每次写入保留精确的前后文本与运行标识，结果卡上可以
//! 整次撤销；写入之后的提示（事实疑点、词表与文种问题）只展示，不再阻止写入。
//!
//! 本模块只有数据和两条纯逻辑：这次写入还能不能撤销、往正文写文本时怎么记撤销点。
//! 写入顺序（先落记录、再换正文）与运行回执消费在 `src/app/pi_jobs.rs`。

use serde::{Deserialize, Serialize};

/// 一次写入的精确前后文本与运行标识。
///
/// 卡片上的「撤销本次写入」只照它把正文换回 `before`，不整篇回滚用户之后的编辑：
/// 正文已经不是 `after` 时就拒绝（`blocked_reason`）。全部字段带 serde 默认值，
/// 旧会话没有这一段照样读得回来。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiWrite {
    /// 这一轮 Pi 运行的标识。空串表示没有可用的运行记录（只影响「同一轮不重复应用」）。
    #[serde(default)]
    pub(crate) run_id: String,
    /// 写入前的正文，逐字节。
    #[serde(default)]
    pub(crate) before: String,
    /// 写入后的正文，逐字节。
    #[serde(default)]
    pub(crate) after: String,
    #[serde(default)]
    pub(crate) applied_at: String,
    /// 交付检查里的事实疑点：照常写入，卡片上留一句给人核对。
    #[serde(default)]
    pub(crate) advisories: Vec<String>,
    /// 用户已经撤销过这次写入。撤销后同一轮回执不会再自动盖回来。
    #[serde(default)]
    pub(crate) undone: bool,
}

impl PiWrite {
    /// 现在能不能撤销；不能撤销时返回给人看的原因。
    ///
    /// 正文写入之后用户继续编辑是常态：那时整篇换回 `before` 会冲掉这些编辑，
    /// 所以卡片撤销禁用并说明，普通编辑器里的 Ctrl+Z 仍按顺序工作。
    pub(crate) fn blocked_reason(&self, document: &str) -> Option<&'static str> {
        if self.undone {
            return Some("这次写入已经撤销过了。");
        }
        if document != self.after {
            return Some(
                "写入之后正文又改过，整篇撤销会冲掉这些编辑；需要逐次撤回请用正文编辑器的 Ctrl+Z。",
            );
        }
        None
    }

    /// 这一轮的成品是不是已经写过（同一份回执不能自动应用第二次，撤销后也不行）。
    pub(crate) fn matches_run(&self, run_id: &str) -> bool {
        !run_id.is_empty() && self.run_id == run_id
    }
}

/// 交付检查已通过、但因正文冲突 / 保存失败 / 只读没能写进正文的成品。
///
/// 留着它就不会丢掉任何一方：当前正文不动，AI 成品在这里，用户按当前正文重做，
/// 或显式「取回 AI 成品」（写入时同样记撤销点，取回本身也可撤销）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiPendingDelivery {
    #[serde(default)]
    pub(crate) run_id: String,
    pub(crate) markdown: String,
    #[serde(default)]
    pub(crate) advisories: Vec<String>,
}

/// Pi 写入差异的视图状态。`DiffViewState` 没有 `Debug`，而轮次要跟 `Debug`；
/// 这里只包一层，不碰对照视图本身。
#[derive(Default)]
pub(crate) struct PiDiffView(pub(crate) crate::diff_view::DiffViewState);

impl std::fmt::Debug for PiDiffView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PiDiffView")
    }
}

/// 把成品写进正文。
///
/// 底层文本替换：调用方先用 bind_editor 绑定对应稿件；正文写入统一经 write_document。
/// 不带 ctx 只用于无界面的回归测试，不修改编辑器内存。
pub(crate) fn write_text(ctx: Option<&eframe::egui::Context>, text: &mut String, after: &str) {
    match ctx {
        Some(ctx) => crate::draft_page::replace_with_undo(ctx, text, after.to_string(), 0),
        None => *text = after.to_string(),
    }
}

/// 共用 editor_id 的状态按稿件保存在 egui 内存中；后台写入也能保留自己的撤销点。
pub(crate) fn bind_editor(ctx: &eframe::egui::Context, key: crate::draft_page::DocKey) {
    use eframe::egui;
    let owner_id = egui::Id::new("pi_editor_owner");
    let previous = ctx.data(|data| data.get_temp::<u64>(owner_id));
    if previous == Some(key) {
        return;
    }
    if let Some(previous) = previous
        && let Some(state) = egui::TextEdit::load_state(ctx, crate::draft_page::editor_id())
    {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new(("pi_editor_state", previous)), state));
    }
    let state = ctx
        .data(|data| {
            data.get_temp::<egui::text_edit::TextEditState>(egui::Id::new(("pi_editor_state", key)))
        })
        .or_else(|| {
            previous
                .is_none()
                .then(|| eframe::egui::TextEdit::load_state(ctx, crate::draft_page::editor_id()))
                .flatten()
        })
        .unwrap_or_default();
    state.store(ctx, crate::draft_page::editor_id());
    ctx.data_mut(|data| data.insert_temp(owner_id, key));
}

fn write_document(
    ctx: Option<&eframe::egui::Context>,
    doc: &mut crate::draft_page::DraftSession,
    after: &str,
) {
    let previous = ctx.and_then(|ctx| {
        ctx.data(|data| data.get_temp::<u64>(eframe::egui::Id::new("pi_editor_owner")))
    });
    if let Some(ctx) = ctx {
        bind_editor(ctx, doc.key);
    }
    write_text(ctx, &mut doc.generated_markdown, after);
    if let (Some(ctx), Some(previous)) = (ctx, previous)
        && previous != doc.key
    {
        bind_editor(ctx, previous);
    }
}

/// 把一份已通过交付检查的成品写进正文，返回「哪一轮用到现在」。
///
/// 顺序不能变：先确认撤销记录能落盘（有库稿件必须先写进会话），再标卡片、消费运行回执，
/// 最后才换正文。落盘失败时就地退回「保留成品」的状态，**正文不动**，不形成无撤销记录的
/// 替换。回执消费失败不回滚写入（会话已经把这一轮标成「已写入」），但要报出来。
pub(crate) fn commit(
    doc: &mut crate::draft_page::DraftSession,
    store: Option<&mut crate::manuscript::ManuscriptStore>,
    ctx: Option<&eframe::egui::Context>,
    turn_id: u64,
    after: String,
    advisories: Vec<String>,
) -> Result<(), String> {
    use crate::ai_panel::TurnState;
    if doc.read_only() {
        return Err("这篇稿件已发布或归档，只读，不能写入 AI 成品。".into());
    }
    let run_id = doc
        .ai_panel
        .turns
        .iter()
        .find(|turn| turn.id == turn_id)
        .and_then(|turn| turn.pi_run.as_ref())
        .map(|run| run.run_id.clone())
        .unwrap_or_default();
    if doc.ai_panel.pi_run_already_written(&run_id) {
        // 同一份成品已经写过：这一轮不再重复应用，卡片说清楚为什么。
        if let Some(turn) = doc.ai_panel.turn_mut(turn_id) {
            turn.pi_pending = None;
            turn.notes
                .push("这一轮的成品已经写进正文了，不再重复应用。".into());
            turn.settle(TurnState::Discarded);
        }
        return Err("这一轮的成品已经写过，不再重复应用。".into());
    }
    // 有库稿件先看会话能不能落盘（与 PiPrepared 同一套前置条件）：
    // 存不了撤销记录就不写正文，成品留在这一轮里等用户处理。
    if let Some(id) = doc.manuscript_id
        && (store.is_none() || doc.ai_panel.session.loaded_for != Some(id))
    {
        let reason = if store.is_none() {
            "稿件库没有打开，无法保存撤销记录；未写入正文，成品留在这一轮里。"
        } else {
            "这篇稿件还没有和稿件库对齐，无法保存撤销记录；请先保存稿件，再点「取回 AI 成品」。"
        };
        park_pending(doc, turn_id, reason);
        return Err(reason.into());
    }
    let write = PiWrite {
        run_id: run_id.clone(),
        before: doc.generated_markdown.clone(),
        after: after.clone(),
        applied_at: chrono::Local::now().to_rfc3339(),
        advisories,
        undone: false,
    };
    if !doc.ai_panel.mark_pi_written(turn_id, write) {
        return Err("这一轮已经不在了。".into());
    }
    let manuscript_id = doc.manuscript_id;
    if let Some(error) = doc
        .ai_panel
        .save_session(manuscript_id, store, None, true)
        .filter(|_| manuscript_id.is_some())
    {
        // 正文还没换：退回「成品已保留」的卡片，别让人以为已经写进去了。
        let reason = format!("{error}；未写入正文");
        park_pending(doc, turn_id, &reason);
        return Err(format!("{reason}，成品留在这一轮里。"));
    }
    // 回执只能应用一次：运行记录标成已写入，重启后重放也不会再自动覆盖。
    let consumed = doc
        .ai_panel
        .turns
        .iter()
        .find(|turn| turn.id == turn_id)
        .and_then(|turn| turn.pi_run.clone());
    let consume_error = consumed
        .as_ref()
        .and_then(|run| crate::pi_agent::consume_run(run).err());
    write_document(ctx, doc, &after);
    // 校对与预览状态跟正文走：旧编译结果的孤行实测与成品列表不再算数。
    doc.proof_markdown.clear();
    doc.proof_warnings.clear();
    doc.output_files.clear();
    doc.export_error = None;
    if doc.ai_panel.workspace_view == Some(turn_id) {
        doc.ai_panel.workspace_view = None;
    }
    if let Some(error) = consume_error
        && let Some(turn) = doc.ai_panel.turn_mut(turn_id)
    {
        turn.notes.push(format!(
            "运行记录没能标成已写入：{error}。会话里已经记下这次写入，卡片上的撤销仍然可用。"
        ));
    }
    Ok(())
}

/// 没能写成时把卡片退回「成品已保留」：正文不动，用户还能显式取回或丢弃。
fn park_pending(doc: &mut crate::draft_page::DraftSession, turn_id: u64, reason: &str) {
    let pending = doc.ai_panel.turn_mut(turn_id).and_then(|turn| {
        let write = turn.pi_write.take();
        if write.is_some()
            && let Some(run) = turn.pi_run.as_mut()
        {
            run.status = crate::pi_agent::PiRunStatus::Delivered;
        }
        turn.pi_pending.clone().or_else(|| {
            write.map(|write| PiPendingDelivery {
                run_id: write.run_id,
                markdown: write.after,
                advisories: write.advisories,
            })
        })
    });
    match pending {
        Some(pending) => {
            doc.ai_panel
                .park_pi_delivery(turn_id, pending, false, reason.to_string());
        }
        None => {
            if let Some(turn) = doc.ai_panel.turn_mut(turn_id) {
                turn.settle(crate::ai_panel::TurnState::Failed(reason.to_string()));
            }
        }
    }
}

/// 撤销一次写入：正文回到写入前的版本。返回需要补报的会话保存问题。
pub(crate) fn undo(
    doc: &mut crate::draft_page::DraftSession,
    store: Option<&mut crate::manuscript::ManuscriptStore>,
    ctx: Option<&eframe::egui::Context>,
    turn_id: u64,
) -> Result<(), String> {
    if doc.read_only() {
        return Err("这篇稿件已发布或归档，只读，不能撤销写入。".into());
    }
    let Some(mut write) = doc
        .ai_panel
        .turns
        .iter()
        .find(|turn| turn.id == turn_id)
        .and_then(|turn| turn.pi_write.clone())
    else {
        return Err("这一轮没有可撤销的写入记录。".into());
    };
    if let Some(reason) = write.blocked_reason(&doc.generated_markdown) {
        return Err(reason.into());
    }
    write_document(ctx, doc, &write.before);
    doc.proof_markdown.clear();
    doc.proof_warnings.clear();
    doc.output_files.clear();
    doc.export_error = None;
    write.undone = true;
    if let Some(turn) = doc.ai_panel.turn_mut(turn_id) {
        turn.pi_write = Some(write);
        turn.notes
            .push("已撤销本次 Pi 写入，正文恢复为写入前的版本。".into());
    }
    let manuscript_id = doc.manuscript_id;
    let error = doc
        .ai_panel
        .save_session(manuscript_id, store, None, true)
        .filter(|_| manuscript_id.is_some());
    if let Some(error) = error {
        return Err(format!(
            "{error}；正文已经恢复，但撤销记录没能存进会话（再次写入会覆盖它）。"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(before: &str, after: &str) -> PiWrite {
        PiWrite {
            run_id: "run-1".into(),
            before: before.into(),
            after: after.into(),
            applied_at: "2026-11-02T10:00:00+08:00".into(),
            advisories: vec!["日期「2026年10月8日」未在本轮依据中找到".into()],
            undone: false,
        }
    }

    #[test]
    fn undo_is_only_allowed_while_the_document_is_still_the_written_one() {
        let record = write("# 通知\n\n原文。\n", "# 通知\n\n改后。\n");
        assert_eq!(record.blocked_reason("# 通知\n\n改后。\n"), None);
        // 写入之后又手工改过：不许整篇盖回去。
        assert!(
            record
                .blocked_reason("# 通知\n\n改后，又加了一句。\n")
                .is_some_and(|reason| reason.contains("Ctrl+Z"))
        );
        let mut undone = record.clone();
        undone.undone = true;
        assert!(
            undone
                .blocked_reason("# 通知\n\n改后。\n")
                .is_some_and(|reason| reason.contains("已经撤销"))
        );
    }

    #[test]
    fn a_write_belongs_to_one_run_and_old_sessions_read_back() {
        let record = write("甲", "乙");
        assert!(record.matches_run("run-1"));
        assert!(!record.matches_run("run-2"));
        assert!(!record.matches_run(""), "没有运行标识时不做同一轮判定");
        // 旧会话没有这段字段：缺省读回为 None，不影响其它字段。
        assert_eq!(
            serde_json::from_str::<Option<PiWrite>>("null").unwrap(),
            None
        );
        // 新记录只带运行标识与文本也要读得回来（其余字段有默认值）。
        let saved: PiWrite =
            serde_json::from_str(r#"{"runId":"run-3","before":"旧","after":"新"}"#).unwrap();
        assert_eq!(saved.before, "旧");
        assert!(!saved.undone);
        assert!(saved.advisories.is_empty());
    }

    #[test]
    fn a_write_into_an_inactive_tab_leaves_the_editor_undo_stack_alone() {
        let ctx = eframe::egui::Context::default();
        let mut text = "# 通知\n\n原文。\n".to_string();
        write_text(None, &mut text, "# 通知\n\n改后。\n");
        assert_eq!(text, "# 通知\n\n改后。\n");
        assert!(
            eframe::egui::TextEdit::load_state(&ctx, crate::draft_page::editor_id()).is_none(),
            "非活动标签不能碰共用的编辑器状态"
        );
    }

    /// 一次 Ctrl+Z 撤回整次 Pi 替换（复用 `diff_editor::replace_with_undo`）。
    #[test]
    fn one_ctrl_z_takes_the_whole_pi_write_back() {
        use eframe::egui;
        let ctx = egui::Context::default();
        let id = crate::draft_page::editor_id();
        let mut text = "# 通知\n\n原文。\n".to_string();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut clock = 0.0;
        let mut frame = |events: Vec<egui::Event>, text: &mut String| {
            clock += 1.0;
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(clock),
                events,
                ..Default::default()
            };
            crate::test_support::run_ui(&ctx, raw, |ui| {
                egui::TextEdit::multiline(text)
                    .id(id)
                    .desired_width(800.0)
                    .show(ui);
            });
        };
        // 先画一帧，再把焦点放进编辑器，然后写入成品。
        frame(Vec::new(), &mut text);
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(Vec::new(), &mut text);
        write_text(Some(&ctx), &mut text, "# 通知\n\n改后。\n");
        assert_eq!(text, "# 通知\n\n改后。\n");
        let undo = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        frame(vec![undo], &mut text);
        assert_eq!(text, "# 通知\n\n原文。\n", "一次撤销要回到 Pi 写入之前");
    }

    #[test]
    fn background_writes_keep_separate_editor_undo_histories() {
        use eframe::egui;
        let ctx = egui::Context::default();
        let config = crate::models::AppConfig::default();
        let mut first = crate::draft_page::DraftSession::blank(41, &config);
        let mut second = crate::draft_page::DraftSession::blank(42, &config);
        first.generated_markdown = "甲原文".into();
        second.generated_markdown = "乙原文".into();
        let mut time = 0.0;
        let mut frame = |doc: &mut crate::draft_page::DraftSession, undo: bool| {
            bind_editor(&ctx, doc.key);
            time += 1.0;
            let events = if undo {
                vec![egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }]
            } else {
                Vec::new()
            };
            crate::test_support::run_ui(
                &ctx,
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::TextEdit::multiline(&mut doc.generated_markdown)
                        .id(crate::draft_page::editor_id())
                        .show(ui);
                },
            );
        };
        frame(&mut first, false);
        ctx.memory_mut(|memory| memory.request_focus(crate::draft_page::editor_id()));
        frame(&mut first, false);
        write_document(Some(&ctx), &mut first, "甲成品");
        write_document(Some(&ctx), &mut second, "乙成品");
        frame(&mut first, true);
        assert_eq!(first.generated_markdown, "甲原文");
        assert_eq!(second.generated_markdown, "乙成品");
        frame(&mut second, true);
        assert_eq!(
            second.generated_markdown, "乙原文",
            "后台任务切回来后也应一次撤销"
        );
        assert_eq!(first.generated_markdown, "甲原文");
    }
}
