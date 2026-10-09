//! 要素建议卡（`docs/element-fill-design.md` 3.3）：动笔前「要素抽取」的建议挂在这一轮
//! 对话上，不挡起草；用户点采纳才经 `element_fields::apply_field` 写表单（红线 2：
//! 写表单只能由界面线程在用户点采纳时执行）。
//!
//! 本文件放卡片的数据与纯逻辑（合并去重、采纳前校验、排序），以及卡片的渲染。
//! 采纳 / 撤销 / 全部采纳的动作在 `skill_job.rs`（要碰 `DraftPage` 的表单与状态栏）。

use super::ui::CardAction;
use crate::element_fields::{FieldId, FieldSuggestion};
use crate::models::{DraftInput, TemplateProfile, VocabularyCategory, VocabularyEntry};
use crate::theme;
use eframe::egui;

/// 建议卡上一行的状态。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Hash, serde::Serialize, serde::Deserialize,
)]
pub(crate) enum FieldCardState {
    /// 待采纳。
    #[default]
    Pending,
    /// 已采纳；`before` / `after` 两份表单快照在，可撤销。
    Adopted,
    /// 已撤销（恢复采纳前的表单）。
    Undone,
    /// 表单已改动：生成建议时的表单值与当前值不同，不覆盖；再点一次「仍然采纳」才写。
    Stale,
}

/// 建议卡上的一行。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct FieldCardRow {
    pub(crate) suggestion: FieldSuggestion,
    #[serde(default)]
    pub(crate) state: FieldCardState,
    /// 采纳前的整份版式要素：撤销时整体还原（互斥剔除、代字带出、承办条目同步的
    /// 连带变化一起撤回）。
    #[serde(default)]
    pub(crate) before: Option<Box<TemplateProfile>>,
    /// 采纳后的整份版式要素：撤销只允许在表单从那以后没再动过时进行
    /// （当前值与 `after` 不同就说明动过了）。
    #[serde(default)]
    pub(crate) after: Option<Box<TemplateProfile>>,
    /// 行内提示：词库校验不过等原因。再次尝试时清掉重写。
    #[serde(default)]
    pub(crate) note: String,
}

impl FieldCardRow {
    pub(crate) fn new(suggestion: FieldSuggestion) -> Self {
        Self {
            suggestion,
            state: FieldCardState::Pending,
            before: None,
            after: None,
            note: String::new(),
        }
    }
}

/// 两路建议（clarify 事件、答题落地）合进同一张卡：
/// - 按（字段，值）去重，同一场次同一建议不重复出；
/// - 同一字段来了更新的建议，替换待采纳 / 表单已改动的旧行；已采纳、已撤销的行
///   保留作记录，新建议另起一行。
pub(crate) fn merge(rows: &mut Vec<FieldCardRow>, incoming: Vec<FieldSuggestion>) {
    for suggestion in incoming {
        let duplicate = rows.iter().any(|row| {
            row.suggestion.field == suggestion.field && row.suggestion.value == suggestion.value
        });
        if duplicate {
            continue;
        }
        if let Some(old) = rows.iter_mut().find(|row| {
            row.suggestion.field == suggestion.field
                && matches!(row.state, FieldCardState::Pending | FieldCardState::Stale)
        }) {
            *old = FieldCardRow::new(suggestion);
        } else {
            rows.push(FieldCardRow::new(suggestion));
        }
    }
}

/// 还没采纳的行（待采纳 + 表单已改动）：交付提案时的提醒、卡头的计数用它。
pub(crate) fn pending_rows(rows: &[FieldCardRow]) -> Vec<&FieldCardRow> {
    rows.iter()
        .filter(|row| matches!(row.state, FieldCardState::Pending | FieldCardState::Stale))
        .collect()
}

/// 多值字段采纳前按词库顺序排一遍，与表单下拉框回写的顺序一致
/// （`widgets::sort_by_vocabulary` 按下拉选项排，这里按词库本身排，两者同源）。
pub(crate) fn sort_by_vocabulary(value: &str, vocabulary: &[VocabularyEntry]) -> String {
    let mut names = crate::models::split_units(value);
    names.sort_by_key(|name| {
        vocabulary
            .iter()
            .position(|entry| entry.canonical.trim() == name.trim())
            .unwrap_or(usize::MAX)
    });
    crate::models::join_units(&names)
}

/// 建议值里已经不在当前词库的项（多值字段按顿号拆开逐项查）：单位字段只对单位、
/// 人员字段只对人员，按规范名称比对（词库在等待期间被改过也拦得住）。
pub(crate) fn missing_in_vocabulary(
    field: FieldId,
    value: &str,
    vocabulary: &[VocabularyEntry],
) -> Vec<String> {
    let category = match field.source() {
        crate::element_fields::Source::Units { .. } => VocabularyCategory::Unit,
        _ => VocabularyCategory::Person,
    };
    crate::models::split_units(value)
        .into_iter()
        .filter(|name| {
            !vocabulary
                .iter()
                .any(|entry| entry.category == category && entry.canonical.trim() == name.trim())
        })
        .collect()
}

/// 采纳前的检查（顺序即方案 3.3 的 a–c；`force` 是「仍然采纳」）。
/// 返回 None 表示可以采纳；Some(（新状态或不变, 行内提示)）表示拦下。
pub(crate) fn adoption_blocker(
    row: &FieldCardRow,
    draft: &DraftInput,
    vocabulary: &[VocabularyEntry],
    force: bool,
) -> Option<String> {
    let field = row.suggestion.field;
    // 文种校验：字段不适用于当前稿件（文种已改等）就不能采纳。
    if !field.applies(draft) {
        return Some(format!(
            "{}不适用于{}：文种改了，这条建议作废。",
            field.label(),
            draft.kind.label()
        ));
    }
    // 过期检测：表单当前值与生成建议时的值不同，标「表单已改动」，不覆盖。
    if !force && field.read(draft) != row.suggestion.previous {
        return Some("表单已改动：不覆盖。仍要采纳请再点一次「仍然采纳」。".into());
    }
    // 词库校验：每一项都仍是当前词库里的规范名称。
    let missing = missing_in_vocabulary(field, &row.suggestion.value, vocabulary);
    if !missing.is_empty() {
        return Some(format!("{}已不在标准词库", missing.join("、")));
    }
    None
}

/// 撤销是否允许：表单从采纳那一刻起没再动过（当前版式要素与采纳后的快照一致）。
/// 动过表单的是这张卡上后采纳的另一条时，提示先撤那一条（逐条倒着撤就能撤回去），
/// 别让人以为只能去表单里手改。
pub(crate) fn undo_blocker(
    rows: &[FieldCardRow],
    index: usize,
    draft: &DraftInput,
) -> Option<String> {
    let Some(after) = rows.get(index).and_then(|row| row.after.as_ref()) else {
        return Some("这条没有可撤销的采纳记录。".into());
    };
    if draft.profile == **after {
        return None;
    }
    let later = rows.iter().enumerate().find(|(other, row)| {
        *other != index
            && row.state == FieldCardState::Adopted
            && row.after.as_deref() == Some(&draft.profile)
    });
    Some(match later {
        Some((_, row)) => format!(
            "之后又采纳了「{}」：先撤销那一条，再撤这一条。",
            row.suggestion.field.label()
        ),
        None => "表单已改动，请在表单里改。".into(),
    })
}

/// 交付提案时的提醒文字；没有待采纳的返回 None。表单已经填成建议值的行
/// （用户在等待期间自己填了）不算待采纳。
///
/// 不冲突的建议交给了起草（正文按它写），冲突的「替换」建议没交（`prepare.rs` 只记
/// 不冲突的），两类分开说，别把冲突的也说成「正文是按它们写的」。
pub(crate) fn proposal_reminder(rows: &[FieldCardRow], draft: &DraftInput) -> Option<String> {
    let pending: Vec<&FieldCardRow> = pending_rows(rows)
        .into_iter()
        .filter(|row| {
            !crate::element_fields::same_value(
                &row.suggestion.field.read(draft),
                &row.suggestion.value,
            )
        })
        .collect();
    let list = |conflict: bool| {
        pending
            .iter()
            .filter(|row| row.suggestion.conflict == conflict)
            .map(|row| format!("{}：{}", row.suggestion.field.label(), row.suggestion.value))
            .collect::<Vec<_>>()
    };
    let (written, conflicting) = (list(false), list(true));
    let mut parts = Vec::new();
    if !written.is_empty() {
        parts.push(format!(
            "要素建议还有 {} 条没采纳（{}），正文是按它们写的",
            written.len(),
            written.join("；")
        ));
    }
    if !conflicting.is_empty() {
        parts.push(format!(
            "{} 条与表单不一致的建议没处理（{}），正文按表单写",
            conflicting.len(),
            conflicting.join("；")
        ));
    }
    (!parts.is_empty()).then(|| parts.join("；另有 "))
}

/// 建议卡：每行字段名、当前表单值 → 建议值、出处；冲突行标「与表单不一致」、按钮写
/// 「替换」；过期行标「表单已改动」、按钮写「仍然采纳」；已采纳的行显示「已采纳 · 撤销」。
pub(super) fn field_card_ui(
    ui: &mut egui::Ui,
    turn_id: u64,
    rows: &[FieldCardRow],
    draft: &DraftInput,
    action: &mut Option<CardAction>,
) {
    let adoptable = rows
        .iter()
        .any(|row| row.state == FieldCardState::Pending && !row.suggestion.conflict);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("要素建议").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if adoptable
                && ui
                    .add(theme::secondary_icon_button(
                        theme::Icon::SquareCheck,
                        "全部采纳",
                    ))
                    .on_hover_text("采纳所有不冲突的建议；与表单不一致的要逐条点")
                    .clicked()
            {
                *action = Some(CardAction::AdoptAllFields(turn_id));
            }
        });
    });
    ui.add_space(4.0);
    for (index, row) in rows.iter().enumerate() {
        let field = row.suggestion.field;
        let stale = row.state == FieldCardState::Stale
            || (row.state == FieldCardState::Pending
                && field.read(draft) != row.suggestion.previous);
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  {} → {}",
                        field.label(),
                        if row.suggestion.previous.trim().is_empty() {
                            "（空）".to_string()
                        } else {
                            row.suggestion.previous.clone()
                        },
                        row.suggestion.value
                    ))
                    .strong(),
                );
            });
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                theme::caption(ui, &row.suggestion.source);
                if row.suggestion.conflict {
                    theme::chip(ui, "与表单不一致", theme::warn(), theme::warn_soft());
                }
                if stale && row.state != FieldCardState::Adopted {
                    theme::chip(ui, "表单已改动", theme::warn(), theme::warn_soft());
                }
            });
            if !row.note.is_empty() {
                ui.add_space(2.0);
                ui.label(egui::RichText::new(&row.note).small().color(theme::warn()));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match row.state {
                        FieldCardState::Pending | FieldCardState::Stale => {
                            let label = if stale {
                                "仍然采纳"
                            } else if row.suggestion.conflict {
                                "替换"
                            } else {
                                "采纳"
                            };
                            if ui.button(label).clicked() {
                                *action = Some(CardAction::AdoptField(turn_id, index, stale));
                            }
                        }
                        FieldCardState::Adopted => {
                            if ui.small_button("撤销").clicked() {
                                *action = Some(CardAction::UndoField(turn_id, index));
                            }
                            theme::chip(ui, "已采纳", theme::success(), theme::success_soft());
                        }
                        FieldCardState::Undone => {
                            theme::chip(ui, "已撤销", theme::text_muted(), theme::surface_sunk());
                        }
                    },
                );
            });
        });
        ui.add_space(4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion(field: FieldId, value: &str) -> FieldSuggestion {
        FieldSuggestion {
            field,
            value: value.into(),
            previous: String::new(),
            source: "材料原文：「测试」".into(),
            conflict: false,
        }
    }

    /// 合并去重：同（字段，值）不重复出；同一字段的新建议替换待采纳的旧行；
    /// 已采纳的旧行保留作记录，新建议另起一行。
    #[test]
    fn merge_dedups_replaces_pending_and_keeps_decided_rows() {
        let mut rows = vec![FieldCardRow::new(suggestion(FieldId::Recipient, "甲局"))];
        // 同（字段，值）再来一遍：不重复。
        merge(&mut rows, vec![suggestion(FieldId::Recipient, "甲局")]);
        assert_eq!(rows.len(), 1);
        // 同一字段换了值：替换待采纳的旧行。
        merge(&mut rows, vec![suggestion(FieldId::Recipient, "乙局")]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].suggestion.value, "乙局");
        // 已采纳的行不替换，新建议另起一行。
        rows[0].state = FieldCardState::Adopted;
        merge(&mut rows, vec![suggestion(FieldId::Recipient, "丙局")]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].state, FieldCardState::Adopted);
        assert_eq!(rows[1].state, FieldCardState::Pending);
        // 「表单已改动」的行同样会被更新的建议替换。
        rows[1].state = FieldCardState::Stale;
        merge(&mut rows, vec![suggestion(FieldId::Recipient, "丁局")]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].suggestion.value, "丁局");
        assert_eq!(rows[1].state, FieldCardState::Pending);
    }

    /// 词库校验逐项拆开查：单位字段只对单位、人员字段只对人员。
    #[test]
    fn missing_vocabulary_items_are_reported_one_by_one() {
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "甲局".into(),
                ..Default::default()
            },
            // 与单位同名的人员不该让单位字段通过，反之亦然。
            VocabularyEntry {
                canonical: "乙局".into(),
                category: VocabularyCategory::Person,
                ..Default::default()
            },
        ];
        assert_eq!(
            missing_in_vocabulary(FieldId::Recipient, "甲局、丙局", &vocabulary),
            ["丙局"]
        );
        assert_eq!(
            missing_in_vocabulary(FieldId::Recipient, "甲局、乙局", &vocabulary),
            ["乙局"],
            "人员类目里的同名词条不算数"
        );
        assert!(missing_in_vocabulary(FieldId::Recipient, "甲局", &vocabulary).is_empty());
    }

    /// 多值字段按词库顺序排，词库外的保持相对顺序排在最后（采纳前的排序）。
    #[test]
    fn multi_values_sort_by_vocabulary_order() {
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "甲局".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "乙局".into(),
                ..Default::default()
            },
        ];
        assert_eq!(sort_by_vocabulary("乙局、甲局", &vocabulary), "甲局、乙局");
    }
}
