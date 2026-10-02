//! 源码编辑框里研究报告的文献引用标记：每组 `[@…]` 下面画一道细虚线，组里有键不在
//! 文献库里的画橙色；指针停在引用上片刻，弹出卡片列出每条文献在参考文献表里的
//! 著录与 PDF 序号。悬停的手感与公文引用（`citation_marks`）一致，画法共用它的
//! 行矩形计算；卡片只看不改，没有操作。

use super::citation_marks::{char_offsets, row_rects};
use crate::export::bibliography::{BibEntry, Library};
use crate::export::crossref::{self, CitedKey};
use crate::theme;
use eframe::egui;
use std::sync::Arc;
use std::time::Duration;

/// 指针停多久才弹卡片，与公文引用一致。
const HOVER_DELAY: f64 = 0.35;

/// 卡片的记忆：指的是哪一组、从什么时候开始悬停、上一帧卡片画在哪。
#[derive(Clone, Copy, Default)]
struct CardState {
    target: Option<(usize, usize)>,
    since: f64,
    shown: bool,
    rect: Option<egui::Rect>,
}

/// 画线并处理悬停卡片。`text` 必须是这一帧编辑框排版用的文字；`cited` 只在卡片
/// 真要画时才调（序号要把全文走一遍）。
pub(super) fn show(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    library: &Library,
    cited: impl FnOnce() -> Arc<Vec<CitedKey>>,
) {
    let state_id = egui::Id::new("gw-bib-card");
    let spans = if text.contains("[@") {
        crossref::citation_spans(text)
    } else {
        Vec::new()
    };
    if spans.is_empty() {
        ui.data_mut(|data| data.remove::<CardState>(state_id));
        return;
    }
    let mut state: CardState = ui.data(|data| data.get_temp(state_id)).unwrap_or_default();
    let painter = ui.painter().with_clip_rect(output.text_clip_rect);
    let pointer = ui.input(|input| input.pointer.hover_pos());
    let dragging = ui.input(|input| input.pointer.any_down());
    let chars = char_offsets(text, spans.iter().map(|(range, _)| range));
    let mut hovered: Option<(usize, egui::Rect)> = None;
    let mut first_rects = Vec::with_capacity(spans.len());
    for (index, ((_, keys), (start, end))) in spans.iter().zip(chars).enumerate() {
        let color = if keys.iter().any(|key| !library.contains(key)) {
            theme::warn()
        } else {
            theme::text_muted().gamma_multiply(0.75)
        };
        let rects = row_rects(&output.galley, output.galley_pos, start..end);
        for rect in &rects {
            let y = rect.bottom() - 1.0;
            painter.extend(egui::Shape::dashed_line(
                &[egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0, color),
                3.0,
                2.0,
            ));
            if pointer.is_some_and(|pos| rect.contains(pos) && output.text_clip_rect.contains(pos))
            {
                hovered = Some((index, *rect));
            }
        }
        first_rects.push(rects.first().copied());
    }

    let now = ui.input(|input| input.time);
    let over_card = pointer.is_some_and(|pos| {
        state
            .rect
            .is_some_and(|rect| state.shown && rect.expand(4.0).contains(pos))
    });
    let key_of = |index: usize| (spans[index].0.start, spans[index].0.end);
    match hovered {
        _ if dragging && !over_card => state = CardState::default(),
        Some((index, _)) if state.target != Some(key_of(index)) => {
            state = CardState {
                target: Some(key_of(index)),
                since: now,
                shown: false,
                rect: None,
            };
        }
        Some(_) => {}
        None if over_card => {}
        None => state = CardState::default(),
    }
    if let Some(target) = state.target
        && let Some((index, anchor)) = hovered.or_else(|| {
            // 指针移到卡片上了：按记下的位置找回那一组。
            let index = (0..spans.len()).find(|&index| key_of(index) == target)?;
            first_rects[index].map(|rect| (index, rect))
        })
    {
        if state.shown || now - state.since >= HOVER_DELAY {
            state.shown = true;
            let cited = cited();
            state.rect = Some(card(ui, anchor, &spans[index].1, library, &cited));
        } else {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(HOVER_DELAY - (now - state.since)));
        }
    }
    ui.data_mut(|data| data.insert_temp(state_id, state));
}

fn card(
    ui: &egui::Ui,
    anchor: egui::Rect,
    keys: &[&str],
    library: &Library,
    cited: &[CitedKey],
) -> egui::Rect {
    egui::Area::new(egui::Id::new("gw-bib-card-area"))
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
                    ui.set_max_width(380.0);
                    ui.label(egui::RichText::new("文献引用").strong());
                    let mut seen: Vec<&str> = Vec::new();
                    for key in keys {
                        if seen.contains(key) {
                            continue;
                        }
                        seen.push(key);
                        ui.add_space(4.0);
                        let cited = cited.iter().find(|cited| cited.key == *key);
                        entry_card(
                            ui,
                            key,
                            cited.map(|cited| cited.number),
                            cited.map_or(0, |cited| cited.uses),
                            library.get(key),
                        );
                    }
                });
        })
        .response
        .rect
}

/// 一条文献的说明卡：序号与引用处数、参考文献表里的著录、源码写法。
/// 源码悬停卡与「文献引用」下拉的悬停共用。
pub(crate) fn entry_card(
    ui: &mut egui::Ui,
    key: &str,
    number: Option<usize>,
    uses: usize,
    entry: Option<&BibEntry>,
) {
    ui.horizontal(|ui| {
        match number {
            Some(number) => ui.label(egui::RichText::new(format!("[{number}]")).strong()),
            None => ui.label(egui::RichText::new("未引").strong()),
        };
        let status = match (entry, uses) {
            (None, _) => "文献库里没有这一条".to_string(),
            (Some(_), 0) => "引用后按出现先后编号".to_string(),
            (Some(_), uses) => format!("正文引用 {uses} 处"),
        };
        ui.label(small(&status).color(if entry.is_none() {
            theme::warn()
        } else {
            theme::text_muted()
        }));
    });
    match entry {
        Some(entry) if !entry.formatted.is_empty() => {
            ui.label(egui::RichText::new(&entry.formatted));
        }
        Some(entry) => {
            ui.label(
                small(&format!(
                    "{}{}",
                    entry.title,
                    if entry.title.is_empty() {
                        "BibTeX 有错，排不出著录"
                    } else {
                        "（BibTeX 有错，排不出完整著录）"
                    }
                ))
                .color(theme::text_soft()),
            );
        }
        None => {
            ui.label(
                small("PDF 里会印成 [?] 并中止导出：检查键名拼写，或把这条补进 .bib。")
                    .color(theme::text_soft()),
            );
        }
    }
    ui.label(small(&format!("[@{key}]")).color(theme::text_muted()));
}

fn small(text: &str) -> egui::RichText {
    egui::RichText::new(text).size(theme::font_sizes::SMALL)
}
