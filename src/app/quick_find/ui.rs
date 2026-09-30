//! 检索框固定在上方，只有输入内容后才向下展开结果。

use super::{QuickFind, Target, view};
use crate::theme;
use eframe::egui;

pub(super) const MAX_RESULTS: usize = 50;
const SEARCH_HEIGHT: f32 = 64.0;

pub(in crate::app) fn search_id() -> egui::Id {
    egui::Id::new("quick_find_query")
}

/// 顶边固定，避免结果数量变化时居中的浮层带着输入框上下跳动。
pub(super) fn modal(ctx: &egui::Context) -> egui::Modal {
    let top = (ctx.content_rect().height() * 0.18).clamp(24.0, 140.0);
    egui::Modal::new(egui::Id::new("quick_find_modal"))
        .area(
            egui::Modal::default_area(egui::Id::new("quick_find_modal"))
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, top)),
        )
        .frame(
            egui::Frame::new()
                .fill(theme::surface())
                .stroke(egui::Stroke::new(1.0, theme::border_strong()))
                .corner_radius(14)
                .inner_margin(0)
                .shadow(theme::float_shadow(if theme::current().dark {
                    75
                } else {
                    32
                })),
        )
        .backdrop_color(egui::Color32::from_black_alpha(if theme::current().dark {
            48
        } else {
            24
        }))
}

impl QuickFind {
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) -> (Option<Target>, bool) {
        let width = (ui.ctx().content_rect().width() - 48.0).clamp(180.0, 620.0);
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        let (up, down, enter) = self.navigation_keys(ui);
        let close = self.search_bar(ui, width);
        self.refresh_results();
        // 空输入不显示默认条目，也不能让回车打开隐藏的第一个结果。
        if self.query.trim().is_empty() {
            return (None, close);
        }
        if !self.results.is_empty() {
            if down {
                self.selected = (self.selected + 1).min(self.results.len() - 1);
            }
            if up {
                self.selected = self.selected.saturating_sub(1);
            }
        }
        view::divider(ui);
        let mut target = if enter { self.selected_target() } else { None };
        egui::Frame::new().inner_margin(8).show(ui, |ui| {
            if let Some(warning) = &self.warning {
                ui.add(
                    egui::Label::new(egui::RichText::new(warning).size(12.0).color(theme::warn()))
                        .wrap(),
                );
                ui.add_space(8.0);
            }
            if self.results.is_empty() {
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("没有找到匹配项")
                            .size(15.0)
                            .color(theme::text_soft()),
                    );
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("试试标题中的几个字、拼音或文号")
                            .size(12.0)
                            .color(view::secondary_text()),
                    );
                });
                ui.add_space(16.0);
            } else {
                // 下方留足页脚和外边距；小窗口仍能滚动浏览结果。
                let remaining =
                    ui.ctx().content_rect().bottom() - ui.next_widget_position().y - 64.0;
                egui::ScrollArea::vertical()
                    .id_salt("quick_find_results")
                    .max_height(remaining.clamp(64.0, 392.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (position, &index) in self.results.iter().enumerate() {
                            let entry = &self.entries[index];
                            let response =
                                view::result_row(ui, entry, &self.query, self.selected == position);
                            if (up || down) && self.selected == position {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                self.selected = position;
                                target = Some(entry.target);
                            }
                            ui.add_space(2.0);
                        }
                    });
            }
        });
        view::divider(ui);
        view::footer(ui, self.results.len());
        (target, close)
    }

    fn search_bar(&mut self, ui: &mut egui::Ui, width: f32) -> bool {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width, SEARCH_HEIGHT), egui::Sense::hover());
        view::search_icon(
            ui,
            egui::pos2(rect.left() + 27.0, rect.center().y),
            theme::text_soft(),
        );
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 50.0, rect.center().y - 17.0),
            egui::pos2(rect.right() - 64.0, rect.center().y + 17.0),
        );
        let response = ui.put(
            text_rect,
            egui::TextEdit::singleline(&mut self.query)
                .id(search_id())
                .hint_text(
                    egui::RichText::new("搜索稿件、单位人员或页面…")
                        .size(20.0)
                        .color(view::secondary_text()),
                )
                .font(egui::FontId::proportional(20.0))
                .text_color(theme::text())
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::symmetric(0, 4))
                .min_size(text_rect.size())
                .vertical_align(egui::Align::Center)
                .desired_width(text_rect.width()),
        );
        if self.focus_search {
            response.request_focus();
            self.focus_search = false;
        }
        response.on_hover_text("支持中文、拼音、首字母与文号；拼音请用英文输入，可按 Shift 切换。");
        let esc_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 30.0, rect.center().y),
            egui::vec2(34.0, 26.0),
        );
        let escape = ui.interact(esc_rect, search_id().with("escape"), egui::Sense::click());
        escape.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "关闭快捷查找")
        });
        view::keycap(ui, esc_rect, "Esc", escape.hovered());
        escape.on_hover_text("关闭快捷查找").clicked()
    }

    fn navigation_keys(&mut self, ui: &egui::Ui) -> (bool, bool, bool) {
        // 系统输入法正在组句或本帧提交文字时，回车只能选字，不能打开结果。
        let mut ime_event = false;
        ui.input(|input| {
            for event in &input.events {
                if let egui::Event::Ime(event) = event {
                    ime_event = true;
                    match event {
                        egui::ImeEvent::Preedit { text, .. } => self.composing = !text.is_empty(),
                        egui::ImeEvent::Commit(_) => self.composing = false,
                        _ => {}
                    }
                }
            }
        });
        ui.input_mut(|input| {
            let up = input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
            let down = input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
            let enter = input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            if self.composing || ime_event {
                // 重复排版也不能让留下的回车或 Esc 误打开条目、关闭浮层。
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                (false, false, false)
            } else {
                (up, down, enter)
            }
        })
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
