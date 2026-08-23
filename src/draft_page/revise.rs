//! 修订建议面板：逐条采纳、定位、忽略与撤销。
//!
//! 这一页取代了原先「只能点一下跳过去、剩下自己改」的做法。词表里本来就区分
//! 了「必错可一键替换」和「疑似只提示」（见 `proofread::Entry::is_replaceable`），
//! 但那个 `replacement` 一路传到界面就被丢掉了，141 条规则里能一键改的部分等于
//! 白写。接上之后，用户看到的是「布署 → 部署」外加一枚采纳按钮。
//!
//! 采纳这条路径上有两道闸门，缺一不可：
//!
//! - **落地前重锚。** `revalidate` 不是每次击键都跑，用户看着建议改了几个字，
//!   span 就偏了。采纳的那一刻按锚点重新定位，锚不上就拒绝，不按旧偏移量下刀。
//! - **落地后自检。** 替换完立刻用确定性规则复扫，只要出现原先没有的必错命中
//!   就整条回滚。词表规则触发不了这一条，但小模型改写会——这道闸门是为它们
//!   先修好的。

use crate::draft_page::DraftPage;
use crate::models::DraftInput;
use crate::proofread::{self, Level, Lexicon};
use crate::proofread_rules;
use crate::revision::{Revision, RevisionId, RevisionState};
use crate::theme;
use eframe::egui;
use std::collections::BTreeMap;

/// 一帧里用户在建议列表上点出的动作，渲染结束后统一执行——循环里借着
/// `&self.doc`，而执行要 `&mut self`。
enum ReviseAction {
    Accept(RevisionId),
    AcceptAll(Level),
    Locate(std::ops::Range<usize>),
    Ignore(RevisionId),
    IgnoreForever(RevisionId),
    Undo,
}

/// 按级别统计必错命中：条目编号 → 次数。
///
/// 用计数而不是布尔：同一条规则原本命中两处、改完变成三处，同样是「引入了新
/// 问题」，只看有没有会漏掉。
fn mustfix_counts(lexicon: &Lexicon, draft: &DraftInput, text: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for note in lexicon
        .check(text)
        .into_iter()
        .chain(proofread_rules::check(draft, text))
    {
        if note.level != Level::MustFix {
            continue;
        }
        *counts.entry(note.entry_id).or_insert(0usize) += 1;
    }
    counts
}

/// 采纳即自检：改完之后不得出现原先没有的必错命中。
pub(crate) fn verify_no_new_mustfix(
    lexicon: &Lexicon,
    draft: &DraftInput,
    old: &str,
    new: &str,
) -> Result<(), String> {
    let before = mustfix_counts(lexicon, draft, old);
    let after = mustfix_counts(lexicon, draft, new);
    for (id, count) in &after {
        if before.get(id).copied().unwrap_or(0) < *count {
            return Err(format!("该建议会引入新的必错问题（{id}），已放弃采纳。"));
        }
    }
    Ok(())
}

fn level_color(level: Level) -> egui::Color32 {
    match level {
        Level::MustFix => theme::danger(),
        Level::Suspect => theme::warn(),
        Level::Hint => theme::text_muted(),
    }
}

fn level_soft(level: Level) -> egui::Color32 {
    match level {
        Level::MustFix => theme::danger_soft(),
        Level::Suspect => theme::warn_soft(),
        Level::Hint => theme::surface_sunk(),
    }
}

impl DraftPage<'_> {
    /// 采纳一条建议。两道闸门都在这里合上。
    pub(crate) fn accept_revision(&mut self, id: RevisionId) {
        let lexicon = proofread::Lexicon::resolved(&self.config.proofread);
        let draft = self.doc.draft.clone();
        // 建议集合与正文都挂在 `self.doc` 上，闸门又要读 `self.config`，
        // 只能先把两者取出来，做完再放回去。
        let mut revisions = std::mem::take(&mut self.doc.revisions);
        let mut markdown = std::mem::take(&mut self.doc.generated_markdown);
        let outcome = revisions.apply(id, &mut markdown, |old, new| {
            verify_no_new_mustfix(&lexicon, &draft, old, new)
        });
        self.doc.generated_markdown = markdown;
        self.doc.revisions = revisions;
        match outcome {
            Ok(label) => *self.status = format!("已采纳：{label}。"),
            Err(error) => *self.status = error,
        }
    }

    /// 批量采纳某一级别里所有给得出改法的建议。
    ///
    /// 逐条走 [`Self::accept_revision`]，闸门一条也不少——批量只是省点击次数，
    /// 不是降低标准。中途被闸门拦下的那条留在列表里，不影响其余。
    pub(crate) fn accept_all(&mut self, level: Level) {
        let ids = self.doc.revisions.actionable_ids(level);
        let total = ids.len();
        let mut done = 0usize;
        for id in ids {
            let before = self.doc.revisions.applied_count();
            self.accept_revision(id);
            if self.doc.revisions.applied_count() > before {
                done += 1;
            }
        }
        *self.status = if done == total {
            format!("已采纳全部 {done} 条「{}」建议。", level.label())
        } else {
            format!(
                "已采纳 {done} 条「{}」建议，另有 {} 条未通过复核，留在列表中。",
                level.label(),
                total - done
            )
        };
    }

    pub(crate) fn ignore_revision(&mut self, id: RevisionId, forever: bool) {
        let Some(key) = self.doc.revisions.ignore(id) else {
            return;
        };
        if forever {
            if !self.config.proofread.ignored.contains(&key) {
                self.config.proofread.ignored.push(key);
            }
            let _ = crate::storage::save(self.config);
            *self.status = "已记下：这类写法以后不再提示。".into();
        } else {
            *self.status = "已忽略该建议，本篇内不再出现。".into();
        }
    }

    pub(crate) fn undo_revision(&mut self) {
        let mut revisions = std::mem::take(&mut self.doc.revisions);
        let outcome = revisions.undo_last(&mut self.doc.generated_markdown);
        self.doc.revisions = revisions;
        match outcome {
            Ok(label) => *self.status = format!("已撤销：{label}。"),
            Err(error) => *self.status = error,
        }
    }

    /// 修订建议列表。放在审校抽屉最上面——它是唯一「点一下就能改好」的那部分。
    pub(crate) fn revisions_ui(&mut self, ui: &mut egui::Ui) {
        let pending = self.doc.revisions.pending_count();
        let applied = self.doc.revisions.applied_count();
        let stale = self.doc.revisions.stale_count();
        if pending == 0 && applied == 0 && stale == 0 {
            return;
        }
        let mut action = None;

        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("修订建议 {pending}"));
            if applied > 0 {
                theme::chip(
                    ui,
                    &format!("本次已采纳 {applied}"),
                    theme::success(),
                    theme::success_soft(),
                );
            }
            // 失效的条目仍然列着，但要说清楚为什么不能点，否则用户只会觉得
            // 按钮坏了。
            if stale > 0 {
                theme::chip(
                    ui,
                    &format!("{stale} 条待重新校验"),
                    theme::text_muted(),
                    theme::surface_sunk(),
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            let must = self.doc.revisions.actionable_ids(Level::MustFix).len();
            if must > 0
                && theme::primary_icon_button(
                    ui,
                    theme::Icon::SquareCheck,
                    &format!("采纳全部必错（{must}）"),
                )
                .on_hover_text("逐条采纳并逐条复核，任何一条引入新问题都会自动回滚")
                .clicked()
            {
                action = Some(ReviseAction::AcceptAll(Level::MustFix));
            }
            if applied > 0
                && ui
                    .add(theme::icon_text_button(theme::Icon::Undo, "撤销上一处"))
                    .on_hover_text(match self.doc.revisions.last_applied_label() {
                        Some(label) => format!("撤销：{label}"),
                        None => "撤销最近一次采纳".to_string(),
                    })
                    .clicked()
            {
                action = Some(ReviseAction::Undo);
            }
        });
        ui.add_space(4.0);

        for item in self.doc.revisions.items() {
            if let Some(picked) = revision_card(ui, item) {
                action = Some(picked);
            }
            ui.add_space(4.0);
        }

        // 改过正文的动作要重跑一遍校验：一次替换可能连带消掉旁边的命中，也可能
        // 露出原先被盖住的问题，只重定位是不够的。批量采纳在循环结束后才重跑，
        // 循环里重建列表会让它自己收集的那批 id 失效。
        match action {
            Some(ReviseAction::Accept(id)) => {
                self.accept_revision(id);
                self.revalidate();
            }
            Some(ReviseAction::AcceptAll(level)) => {
                self.accept_all(level);
                self.revalidate();
            }
            Some(ReviseAction::Undo) => {
                self.undo_revision();
                self.revalidate();
            }
            Some(ReviseAction::Locate(span)) => self.jump_to_source(span),
            Some(ReviseAction::Ignore(id)) => self.ignore_revision(id, false),
            Some(ReviseAction::IgnoreForever(id)) => self.ignore_revision(id, true),
            None => {}
        }
    }
}

/// 一条建议的卡片。抽屉只有 300 点上下，所以分三行排：级别与来源、改法、理由。
fn revision_card(ui: &mut egui::Ui, item: &Revision) -> Option<ReviseAction> {
    let mut action = None;
    let stale = item.state == RevisionState::Stale;
    theme::card()
        .fill(if stale {
            theme::surface_sunk()
        } else {
            level_soft(item.severity)
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                theme::chip(
                    ui,
                    item.severity.label(),
                    level_color(item.severity),
                    theme::surface(),
                );
                ui.weak(item.group.as_str());
                ui.weak("·");
                ui.weak(item.source.label());
                // 模型来源要一眼看得出来。词表命中是确定的，模型判断不是——
                // 两者混在一列里而不加区分，用户迟早会把模型的猜测当规则来信。
                if item.confidence < 1.0 {
                    theme::chip(ui, "需人工判断", theme::warn(), theme::surface());
                }
                if stale {
                    ui.colored_label(theme::text_muted(), "原文已改动");
                }
            });

            // 改法一行显示成「原文 → 建议」，用户不必读完整句理由就能判断。
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(item.before())
                        .strikethrough()
                        .color(theme::danger()),
                );
                if let Some(after) = &item.after {
                    ui.label(egui::RichText::new("→").color(theme::text_muted()));
                    ui.label(
                        egui::RichText::new(after.as_str())
                            .strong()
                            .color(theme::success()),
                    );
                }
            });

            ui.add(
                egui::Label::new(
                    egui::RichText::new(item.reason.as_str()).color(theme::text_soft()),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );

            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                if item.after.is_some() {
                    if theme::primary_icon_button_enabled(
                        ui,
                        item.is_actionable(),
                        theme::Icon::SquareCheck,
                        "采纳",
                    )
                    .on_disabled_hover_text("正文已改动，请重新校验后再采纳")
                    .clicked()
                    {
                        action = Some(ReviseAction::Accept(item.id));
                    }
                } else {
                    // 「删去『通过』或『使』」这类建议只能人来改，说清楚比给一个
                    // 灰按钮强——灰按钮只会让人反复去点。
                    ui.weak("需人工改写");
                }
                // 失效条目的 span 是上一次校验留下的旧坐标，跳过去多半落在别的
                // 地方——比不给跳更糟，所以一并禁用。
                if ui
                    .add_enabled(!stale, theme::icon_text_button(theme::Icon::Reveal, "定位"))
                    .on_hover_text("跳到正文中的这一处")
                    .on_disabled_hover_text("正文已改动，位置不再准确，请重新校验")
                    .clicked()
                {
                    action = Some(ReviseAction::Locate(item.span.clone()));
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::X, "忽略"))
                    .on_hover_text("本篇内不再提示这一处")
                    .clicked()
                {
                    action = Some(ReviseAction::Ignore(item.id));
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::Eraser, "不再提示"))
                    .on_hover_text("这类写法以后都不再提示，记在本机配置里")
                    .clicked()
                {
                    action = Some(ReviseAction::IgnoreForever(item.id));
                }
            });
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TemplateKind;

    fn draft() -> DraftInput {
        DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        }
    }

    #[test]
    fn clean_replacement_passes_the_gate() {
        let lexicon = proofread::Lexicon::resolved(&Default::default());
        let result =
            verify_no_new_mustfix(&lexicon, &draft(), "按上级布署办理。", "按上级部署办理。");
        assert!(result.is_ok(), "把错别字改对不该被闸门拦下：{result:?}");
    }

    #[test]
    fn a_replacement_that_introduces_a_typo_is_rejected() {
        // 模拟模型改写把一处错别字换成了另一处：必须整条回滚。
        let lexicon = proofread::Lexicon::resolved(&Default::default());
        let result = verify_no_new_mustfix(
            &lexicon,
            &draft(),
            "按上级安排办理。",
            "按上级布署撤消办理。",
        );
        assert!(result.is_err(), "引入新必错的改写必须被拦下");
    }

    #[test]
    fn removing_one_of_two_hits_still_passes() {
        // 命中数只减不增，闸门不该误伤。
        let lexicon = proofread::Lexicon::resolved(&Default::default());
        let result = verify_no_new_mustfix(&lexicon, &draft(), "先布署再布署。", "先部署再布署。");
        assert!(result.is_ok(), "减少命中不该被拦下：{result:?}");
    }
}
