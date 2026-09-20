//! 候选窗：贴光标画一行候选 + 拼音串。
//!
//! 应用内输入法没有系统候选窗可用（系统输入法已经被关掉了），所以这一块自己画：
//! 拼音串与它的光标位置、当前页的候选、页码。
//!
//! 中英模式不画在这里：状态栏已经常驻显示（见 `app::chrome`），候选窗再挂一个既重复，
//! 又把一个全局状态混进了这一次组字的信息里。
//!
//! 上屏走 [`Ime::pending_commit`]：点击发生在本帧事件处理之后，文本框这一帧已经画完了，
//! 只能把文本排到下一帧的事件队列最前面（见 `session::Ime::begin_frame`）。

use eframe::egui;

use super::keys::Action;
use super::session::{Ime, Preedit};
use crate::theme;

/// 候选窗离光标多远。
const GAP: f32 = 6.0;

/// 没量到尺寸时先按这么大摆（第一帧）。
const FALLBACK_SIZE: egui::Vec2 = egui::vec2(200.0, 46.0);

/// 候选块的内边距。纵向只给 1px：高亮块贴着字走，行才不会被撑开。
const CELL_PADDING: egui::Vec2 = egui::vec2(4.0, 1.0);

/// 候选块之间的间隙。加上两边的内边距，两个候选的字距是 10px。
const CELL_GAP: f32 = 2.0;

/// 拼音串与候选行之间的间隙。
const ROW_GAP: f32 = 2.0;

/// 拼音串与页码之间至少留这么宽：候选行很窄时两者不至于挤到一块。
const HEADER_GAP: f32 = 10.0;

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
        let position = self.window_position(ctx, anchor);
        let rows_width = self.rows_width().unwrap_or(0.0);
        let mut clicked = None;
        let mut measured = 0.0;
        let area = egui::Area::new(egui::Id::new("gw-ime-candidates"))
            .order(egui::Order::Tooltip)
            .fixed_pos(position)
            .constrain_to(ctx.viewport_rect())
            .interactable(true)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::surface())
                    .stroke(egui::Stroke::new(1.0, theme::border()))
                    .corner_radius(8)
                    .inner_margin(egui::Margin {
                        left: 9,
                        right: 9,
                        top: 4,
                        bottom: 5,
                    })
                    // 投影跟主题走：深色底上写死的淡黑托不起窗口。
                    .shadow(ui.style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        // 两行：上面拼音串与页码，下面候选。挤在一行时拼音、候选、页码
                        // 混作一团，读的人分不清哪个是打的、哪个是选的。
                        //
                        // 两行之间不画分隔线：一条线加上下留白要吃掉近 10px，而字号
                        // （12/14）与颜色（弱化/正文）已经把两行分得很开了。
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                            header(ui, &preedit, page, pages, rows_width);
                            ui.add_space(ROW_GAP);
                            measured = candidates_row(ui, &rows, highlight, &mut clicked);
                        });
                    });
            });
        self.remember_window(area.response.rect.size());
        self.remember_rows_width(measured);
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

/// 表头：左边拼音串，右边页码（只有一页就不画）。
fn header(ui: &mut egui::Ui, preedit: &Preedit, page: usize, pages: usize, rows_width: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        pinyin_strip(ui, preedit);
        if pages <= 1 {
            return;
        }
        let text = format!("{}/{}", page + 1, pages);
        let galley = ui.painter().layout_no_wrap(
            text.clone(),
            egui::FontId::proportional(theme::font_sizes::SMALL),
            theme::text_muted(),
        );
        // 页码顶到右边：按上一帧量到的候选行宽度算空档。候选行的宽度不受表头影响，
        // 两者不会互相牵扯；候选变窄时页码最多晚一帧跟上来。
        let space = rows_width - ui.min_rect().width() - galley.size().x;
        ui.add_space(space.max(HEADER_GAP));
        ui.label(
            egui::RichText::new(text)
                .size(theme::font_sizes::SMALL)
                .color(theme::text_muted()),
        );
    });
}

/// 候选行。返回量到的宽度，下一帧的表头拿它摆页码。
fn candidates_row(
    ui: &mut egui::Ui,
    rows: &[(usize, String)],
    highlight: usize,
    clicked: &mut Option<usize>,
) -> f32 {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = CELL_GAP;
        ui.spacing_mut().button_padding = CELL_PADDING;
        // 没选中的候选不描边、不上底，只在鼠标底下才浮出一块淡底。
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        widgets.hovered.weak_bg_fill = theme::surface_hover();
        widgets.active.weak_bg_fill = theme::surface_active();
        widgets.hovered.bg_stroke = egui::Stroke::NONE;
        widgets.active.bg_stroke = egui::Stroke::NONE;
        // 悬停时不让候选块胀大：一胀整行就跟着抖。
        widgets.hovered.expansion = 0.0;
        widgets.active.expansion = 0.0;
        for (index, text) in rows {
            if candidate_button(ui, *index, text, *index == highlight).clicked() {
                *clicked = Some(*index);
            }
        }
    })
    .response
    .rect
    .width()
}

/// 拼音串：光标位置用一条竖线标出来。字号比候选小一档、颜色弱一档，
/// 一眼就分得清哪一行是打进去的、哪一行是要选的。
fn pinyin_strip(ui: &mut egui::Ui, preedit: &Preedit) {
    let chars: Vec<char> = preedit.text.chars().collect();
    let caret = preedit.caret.min(chars.len());
    let before: String = chars[..caret].iter().collect();
    let after: String = chars[caret..].iter().collect();
    let small = |text: String, color: egui::Color32| {
        egui::RichText::new(text)
            .size(theme::font_sizes::SMALL)
            .color(color)
    };
    ui.label(small(before, theme::text_muted()));
    ui.label(small("|".into(), theme::accent()));
    ui.label(small(after, theme::text_muted()));
}

/// 一个候选：弱化的小号序号 + 正文字号的候选文本。
fn candidate_button(ui: &mut egui::Ui, index: usize, text: &str, selected: bool) -> egui::Response {
    let format = |size: f32, color: egui::Color32| egui::TextFormat {
        font_id: egui::FontId::proportional(size),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &(index + 1).to_string(),
        0.0,
        format(
            theme::font_sizes::SMALL,
            if selected {
                // 选中的序号跟着高亮走，但要比候选本身淡：它只是个按键提示。
                theme::accent_active().gamma_multiply(0.7)
            } else {
                theme::text_muted()
            },
        ),
    );
    job.append(
        text,
        3.0,
        format(
            theme::font_sizes::BODY,
            if selected {
                theme::accent_active()
            } else {
                theme::text()
            },
        ),
    );
    let button = egui::Button::new(job)
        .stroke(egui::Stroke::NONE)
        .corner_radius(4);
    ui.add(if selected {
        button.fill(theme::accent_soft())
    } else {
        button
    })
}
