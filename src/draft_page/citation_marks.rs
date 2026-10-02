//! 源码编辑框里的公文引用标记：识别出的《名称》（文号）下面画一道细虚线，
//! 写法不规范、与来源不一致或来源已变的画橙色；指针停在引用上片刻，弹出卡片说明
//! 状态并给出操作。卡片里的操作交回编辑器统一执行，这里只画、只收集。

use super::references::{CitationIndex, Group, Match, SourceKey};
use crate::theme;
use eframe::egui;
use std::ops::Range;
use std::time::Duration;

/// 指针停多久才弹卡片：路过不打扰。
const HOVER_DELAY: f64 = 0.35;

/// 卡片里点的操作。
pub(crate) enum CardAction {
    /// 把这些位置的文字换成新写法。
    Rewrite(Vec<(Range<usize>, String)>),
    Register(String, String),
    OpenSource(i64),
    /// 打开公文引用面板看全部。
    OpenPanel,
}

/// 卡片的记忆：指的是哪一处引用、从什么时候开始悬停、上一帧卡片画在哪。
#[derive(Clone, Copy, Default)]
struct CardState {
    target: Option<(usize, usize)>,
    since: f64,
    shown: bool,
    rect: Option<egui::Rect>,
}

/// 画线并处理悬停卡片。`text` 必须是这一帧编辑框排版用的文字。
pub(crate) fn show(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    index: &CitationIndex,
    editable: bool,
) -> Option<CardAction> {
    let groups = index.groups(text);
    let state_id = egui::Id::new("gw-citation-card");
    let mut state: CardState = ui.data(|data| data.get_temp(state_id)).unwrap_or_default();
    if groups.is_empty() {
        ui.data_mut(|data| data.remove::<CardState>(state_id));
        return None;
    }
    let painter = ui.painter().with_clip_rect(output.text_clip_rect);
    let pointer = ui.input(|input| input.pointer.hover_pos());
    let dragging = ui.input(|input| input.pointer.any_down());
    let mut hovered = None;
    let ranges = groups
        .iter()
        .enumerate()
        .flat_map(|(group, item)| item.ranges.iter().map(move |range| (group, range.clone())))
        .collect::<Vec<_>>();
    let chars = char_offsets(text, ranges.iter().map(|(_, range)| range));
    for ((group_index, range), (start, end)) in ranges.iter().zip(chars) {
        let group = &groups[*group_index];
        let warning = matches!(group.status, Match::Outdated(_) | Match::Differs(_))
            || group.nonstandard.contains(range);
        let color = if warning {
            theme::warn()
        } else {
            theme::text_muted().gamma_multiply(0.75)
        };
        for rect in row_rects(&output.galley, output.galley_pos, start..end) {
            let y = rect.bottom() - 1.0;
            painter.extend(egui::Shape::dashed_line(
                &[egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0, color),
                3.0,
                2.0,
            ));
            if pointer.is_some_and(|pos| rect.contains(pos) && output.text_clip_rect.contains(pos))
            {
                hovered = Some((*group_index, range.clone(), rect));
            }
        }
    }

    let now = ui.input(|input| input.time);
    let over_card = pointer.is_some_and(|pos| {
        state
            .rect
            .is_some_and(|rect| state.shown && rect.expand(4.0).contains(pos))
    });
    match &hovered {
        _ if dragging && !over_card => state = CardState::default(),
        Some((_, range, _)) if state.target != Some((range.start, range.end)) => {
            state = CardState {
                target: Some((range.start, range.end)),
                since: now,
                shown: false,
                rect: None,
            };
        }
        Some(_) => {}
        None if over_card => {}
        None => state = CardState::default(),
    }
    let mut action = None;
    if let Some((start, end)) = state.target
        && let Some((group_index, range, anchor)) = hovered.clone().or_else(|| {
            // 指针移到卡片上了：按记下的位置找回那一处。
            ranges
                .iter()
                .position(|(_, range)| (range.start, range.end) == (start, end))
                .and_then(|at| {
                    let (group, range) = ranges[at].clone();
                    let (start, end) = char_offsets(text, [&range].into_iter())[0];
                    row_rects(&output.galley, output.galley_pos, start..end)
                        .first()
                        .map(|rect| (group, range, *rect))
                })
        })
    {
        if state.shown || now - state.since >= HOVER_DELAY {
            state.shown = true;
            let (rect, picked) = card(ui, anchor, index, &groups[group_index], range, editable);
            state.rect = Some(rect);
            action = picked;
        } else {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(HOVER_DELAY - (now - state.since)));
        }
    }
    if action.is_some() {
        state = CardState::default();
    }
    ui.data_mut(|data| data.insert_temp(state_id, state));
    action
}

fn card(
    ui: &egui::Ui,
    anchor: egui::Rect,
    index: &CitationIndex,
    group: &Group,
    range: Range<usize>,
    editable: bool,
) -> (egui::Rect, Option<CardAction>) {
    let source = index.source(group.status);
    let text = group.text();
    let nonstandard = group.nonstandard.contains(&range);
    let mut action = None;
    let area = egui::Area::new(egui::Id::new("gw-citation-card-area"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom() + egui::vec2(0.0, 4.0))
        .constrain(true)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::surface())
                .stroke(egui::Stroke::new(1.0, theme::border_strong()))
                .corner_radius(egui::CornerRadius::same(theme::PANE_RADIUS))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .shadow(theme::float_shadow(if theme::current().dark {
                    75
                } else {
                    32
                }))
                .show(ui, |ui| {
                    ui.set_max_width(360.0);
                    let small =
                        |text: &str| egui::RichText::new(text).size(theme::font_sizes::SMALL);
                    let (status, warning) = match group.status {
                        Match::Exact(_) => (
                            format!("已登记 · {}", source.map_or("", |s| s.origin.as_str())),
                            false,
                        ),
                        Match::Outdated(_) => ("来源已变".to_owned(), true),
                        Match::Differs(_) => ("与来源不一致".to_owned(), true),
                        Match::Unregistered => ("未登记".to_owned(), false),
                    };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("公文引用").strong());
                        ui.label(small(&status).color(if warning {
                            theme::warn()
                        } else {
                            theme::text_muted()
                        }));
                    });
                    if nonstandard {
                        ui.label(small(&format!("写法不规范，应为 {text}")).color(theme::warn()));
                    }
                    match (group.status, source) {
                        (Match::Outdated(_), Some(source)) => {
                            ui.label(small(&format!("登记已更正为 {}", source.text())));
                        }
                        (Match::Differs(_), Some(source)) => {
                            ui.label(small(&format!("来源为 {}", source.text())));
                        }
                        (Match::Unregistered, _) => {
                            ui.label(
                                small("稿件库和公文登记簿里都没有这份文件。")
                                    .color(theme::text_muted()),
                            );
                        }
                        _ => {}
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let mut link = |ui: &mut egui::Ui, label: &str, picked: CardAction| {
                            if ui.add(egui::Link::new(small(label))).clicked() {
                                action = Some(picked);
                            }
                        };
                        if editable && nonstandard {
                            link(
                                ui,
                                "改为规范写法",
                                CardAction::Rewrite(vec![(range.clone(), text.clone())]),
                            );
                        }
                        if editable
                            && let (Match::Outdated(_) | Match::Differs(_), Some(source)) =
                                (group.status, source)
                        {
                            let rewrite = group
                                .ranges
                                .iter()
                                .map(|range| (range.clone(), source.text()))
                                .collect();
                            link(ui, "改为来源写法", CardAction::Rewrite(rewrite));
                        }
                        if editable
                            && group.status == Match::Unregistered
                            && !group.number.is_empty()
                        {
                            link(
                                ui,
                                "登记到公文登记簿",
                                CardAction::Register(group.title.clone(), group.number.clone()),
                            );
                        }
                        if let Some(SourceKey::Manuscript(id)) = source.map(|source| source.key) {
                            link(ui, "打开来源稿件", CardAction::OpenSource(id));
                        }
                        link(ui, "全部引用", CardAction::OpenPanel);
                    });
                });
        });
    (area.response.rect, action)
}

/// 一组字节范围换成字符下标（galley 按字符计）。先按起点排好，从前往后只数一遍，
/// 长文每帧重算也不至于反复从头数；结果按传入顺序返回。
fn char_offsets<'a>(
    text: &str,
    ranges: impl Iterator<Item = &'a Range<usize>>,
) -> Vec<(usize, usize)> {
    let ranges = ranges.collect::<Vec<_>>();
    let mut order = (0..ranges.len()).collect::<Vec<_>>();
    order.sort_by_key(|&index| ranges[index].start);
    let mut result = vec![(0, 0); ranges.len()];
    let (mut byte, mut chars) = (0, 0);
    for index in order {
        let range = ranges[index];
        chars += text[byte..range.start].chars().count();
        byte = range.start;
        result[index] = (chars, chars + text[range.clone()].chars().count());
    }
    result
}

/// 字符范围落在 galley 各行上的矩形（屏幕坐标），换行处切成几段。
fn row_rects(galley: &egui::Galley, origin: egui::Pos2, chars: Range<usize>) -> Vec<egui::Rect> {
    let mut rects = Vec::new();
    let mut row_start = 0;
    for row in &galley.rows {
        let count = row.char_count_excluding_newline().0;
        let from = chars.start.max(row_start);
        let to = chars.end.min(row_start + count);
        if from < to {
            let left = row.pos.x + row.x_offset(egui::text::CharIndex(from - row_start));
            let right = row.pos.x + row.x_offset(egui::text::CharIndex(to - row_start));
            rects.push(
                egui::Rect::from_min_max(
                    egui::pos2(left, row.min_y()),
                    egui::pos2(right, row.max_y()),
                )
                .translate(origin.to_vec2()),
            );
        }
        row_start += row.char_count_including_newline().0;
        if row_start >= chars.end {
            break;
        }
    }
    rects
}

/// 把卡片操作里的改写落到文字上，返回新文字与光标位置。
pub(crate) fn rewrite(text: &str, mut edits: Vec<(Range<usize>, String)>) -> (String, usize) {
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut updated = text.to_owned();
    for (range, replacement) in &edits {
        updated.replace_range(range.clone(), replacement);
    }
    let cursor = edits
        .last()
        .map_or(0, |(range, replacement)| range.start + replacement.len());
    (updated, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_reference;

    #[test]
    fn rewrite_replaces_from_the_back_and_keeps_offsets() {
        let text = "甲《一》(某办函[2026]1号)乙《一》(某办函[2026]1号)丙";
        let ranges = document_reference::detect_numbered(text)
            .into_iter()
            .map(|citation| (citation.range.clone(), citation.text()))
            .collect::<Vec<_>>();
        let (updated, cursor) = rewrite(text, ranges);
        assert_eq!(
            updated,
            "甲《一》（某办函〔2026〕1号）乙《一》（某办函〔2026〕1号）丙"
        );
        assert_eq!(&updated[cursor..cursor + "乙".len()], "乙");
    }
}
