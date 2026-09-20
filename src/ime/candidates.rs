//! 候选窗：贴光标画一行候选 + 拼音串。
//!
//! 应用内输入法没有系统候选窗可用（系统输入法已经被关掉了），所以这一块自己画：
//! 拼音串与它的光标位置、当前页的候选、页码、中英模式。
//!
//! 上屏走 [`Ime::pending_commit`]：点击发生在本帧事件处理之后，文本框这一帧已经画完了，
//! 只能把文本排到下一帧的事件队列最前面（见 `session::Ime::begin_frame`）。

use eframe::egui;

use super::keys::Action;
use super::session::Ime;
use crate::theme;

/// 候选窗离光标多远。
const GAP: f32 = 6.0;

/// 没量到尺寸时先按这么大摆（第一帧）。
const FALLBACK_SIZE: egui::Vec2 = egui::vec2(240.0, 34.0);

impl Ime {
    /// 画候选窗。要在正文编辑框画完之后调用。
    pub(crate) fn candidates_ui(&mut self, ctx: &egui::Context) {
        if !self.active() {
            return;
        }
        let Some(anchor) = self.anchor else {
            return;
        };
        if self.preedit.text.is_empty() {
            self.forget_window();
            return;
        }
        let page_size = self.settings.page_size.max(1);
        let page = self.highlight / page_size;
        let pages = self.layout.pages().max(1);
        // 先把要画的东西抄成自己的数据：闭包里还要改 `self`（记下点中的候选）。
        let rows: Vec<(usize, String)> = self
            .layout
            .page(page)
            .into_iter()
            .enumerate()
            .filter_map(|(offset, cell)| {
                cell.candidate()
                    .map(|candidate| (offset, candidate.text.clone()))
            })
            .collect();
        let preedit = self.preedit.clone();
        let highlight = self.highlight.saturating_sub(page * page_size);
        let english = self.english;
        let position = self.window_position(ctx, anchor);
        let mut clicked = None;
        let area = egui::Area::new(egui::Id::new("gw-ime-candidates"))
            .order(egui::Order::Tooltip)
            .fixed_pos(position)
            .constrain_to(ctx.viewport_rect())
            .interactable(true)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::surface())
                    .stroke(egui::Stroke::new(1.0, theme::border()))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 2],
                        blur: 10,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(40),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            pinyin_strip(ui, &preedit);
                            ui.add_space(2.0);
                            ui.separator();
                            ui.add_space(2.0);
                            for (index, text) in &rows {
                                if candidate_button(ui, *index, text, *index == highlight).clicked()
                                {
                                    clicked = Some(*index);
                                }
                            }
                            if pages > 1 {
                                ui.add_space(2.0);
                                ui.separator();
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(format!("{}/{}", page + 1, pages))
                                        .color(theme::text_muted()),
                                );
                            }
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(if english { "英" } else { "中" }).color(
                                    if english {
                                        theme::text_muted()
                                    } else {
                                        theme::accent()
                                    },
                                ),
                            );
                        });
                    });
            });
        self.remember_window(area.response.rect.size());
        if let Some(index) = clicked {
            self.commit_clicked(index);
            // 点候选不该把编辑框的焦点带走。
            if let Some(id) = self.focus_id {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
    }

    /// 候选窗摆在光标下方；下方放不下就翻到上方。
    fn window_position(&self, ctx: &egui::Context, anchor: egui::Rect) -> egui::Pos2 {
        let size = self.window_size().unwrap_or(FALLBACK_SIZE);
        let below = anchor.left_bottom() + egui::vec2(0.0, GAP);
        let screen = ctx.viewport_rect();
        let fits_below = below.y + size.y <= screen.bottom();
        let above = anchor.top() - size.y - GAP;
        if !fits_below && above >= screen.top() {
            egui::pos2(below.x, above)
        } else {
            below
        }
    }

    /// 点中一个候选：上屏并排到下一帧的事件队列。
    fn commit_clicked(&mut self, page_index: usize) {
        let outcome = self.execute_guarded(Action::CommitIndex(page_index));
        if let Some(text) = outcome.commit {
            self.pending_commit = Some(text);
            self.refresh(true);
        }
    }
}

/// 拼音串：光标位置用一条竖线标出来。
fn pinyin_strip(ui: &mut egui::Ui, preedit: &super::session::Preedit) {
    let chars: Vec<char> = preedit.text.chars().collect();
    let caret = preedit.caret.min(chars.len());
    let before: String = chars[..caret].iter().collect();
    let after: String = chars[caret..].iter().collect();
    let style = |text: String| egui::RichText::new(text).color(theme::text_soft());
    ui.label(style(before));
    ui.label(egui::RichText::new("|").color(theme::accent()));
    ui.label(style(after));
}

/// 一个候选：序号 + 文本。
fn candidate_button(ui: &mut egui::Ui, index: usize, text: &str, selected: bool) -> egui::Response {
    let label = format!("{} {}", index + 1, text);
    let text = egui::RichText::new(label).color(if selected {
        theme::accent_active()
    } else {
        theme::text()
    });
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::accent_soft()
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::NONE)
            .corner_radius(4),
    )
}
