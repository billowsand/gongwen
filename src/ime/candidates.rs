//! 候选窗：贴光标画一行（或竖排一列）候选 + 编码串。
//!
//! 应用内输入法没有系统候选窗可用（系统输入法已经被关掉了），所以这一块自己画：
//! 编码串与它的光标位置、当前页的候选、页码。
//!
//! 中英模式不画在这里：状态栏已经常驻显示（见 `app::chrome`），候选窗再挂一个既重复，
//! 又把一个全局状态混进了这一次组字的信息里。
//!
//! 上屏走 [`Ime::pending_commit`]：点击发生在本帧事件处理之后，文本框这一帧已经画完了，
//! 只能把文本排到下一帧的事件队列最前面（见 `session::Ime::begin_frame`）。

use eframe::egui;

use super::keys::Action;
use super::session::{Hint, Ime, Preedit};
use crate::theme;

/// 候选窗离光标多远。
const GAP: f32 = 6.0;

/// 没量到尺寸时先按这么大摆（第一帧）。
const FALLBACK_SIZE: egui::Vec2 = egui::vec2(200.0, 46.0);

/// 候选块的内边距。纵向只给 1px：高亮块贴着字走，行才不会被撑开。
const CELL_PADDING: egui::Vec2 = egui::vec2(4.0, 1.0);

/// 候选块之间的间隙。加上两边的内边距，两个候选的字距是 10px。
const CELL_GAP: f32 = 2.0;

/// 编码串与候选行之间的间隙。
const ROW_GAP: f32 = 2.0;

/// 编码串与页码之间至少留这么宽：候选行很窄时两者不至于挤到一块。
const HEADER_GAP: f32 = 10.0;

/// 编码串与「空码」标记的间距。
const EMPTY_GAP: f32 = 6.0;

/// 序号与候选词之间的间距。
const INDEX_GAP: f32 = 3.0;

/// 词表星标边长。
const SPARKLE_SIZE: f32 = 7.0;

/// 词表星标与前面候选词的间距。
const SPARKLE_GAP: f32 = 1.5;

/// 星标腰身收进去的程度：控制点离中心的距离相对半径的比例，越小尖越细。
const SPARKLE_WAIST: f32 = 0.18;

/// 词后右上角小字（简码、逐码提示）与前面候选词 / 星标的间距。
const CORNER_GAP: f32 = 1.0;

/// 候选窗的排法与字号，设置页里来。字号按比例放大正文与小字两档，
/// 星标这些画出来的记号跟着放大，间距不动。
#[derive(Debug, Clone, Copy)]
struct Look {
    vertical: bool,
    scale: f32,
}

impl Look {
    fn small(self) -> f32 {
        theme::font_sizes::SMALL * self.scale
    }

    fn body(self) -> f32 {
        theme::font_sizes::BODY * self.scale
    }

    fn sparkle(self) -> f32 {
        SPARKLE_SIZE * self.scale
    }
}

/// 候选窗中的序号、文字、来源星标与词后小字。
struct Row {
    index: usize,
    text: String,
    /// 来自公文词表、导入表或个人词条（不只在基础表里）。
    starred: bool,
    corner: Option<String>,
    tooltip: String,
}

/// 在候选上做的事：左键上屏，右键菜单里调序、屏蔽、查码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Commit,
    Top,
    Up,
    Down,
    Block,
    Lookup,
}

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
        // 页数按设置里的每页格数算；空布局也算一页。
        let pages = self.layout.len().div_ceil(page_size).max(1);
        // 先把要画的东西抄成自己的数据：闭包里还要改 `self`（记下点中的候选）。
        let rows: Vec<Row> = self
            .layout
            .iter()
            .enumerate()
            .skip(page * page_size)
            .take(page_size)
            .enumerate()
            .map(|(offset, (absolute, slot))| {
                let position = if absolute < self.exact {
                    format!("第 {} 位", absolute + 1)
                } else {
                    "逐码提示".to_string()
                };
                Row {
                    index: offset,
                    text: slot
                        .text
                        .replace(['\n', '\r'], " ")
                        .chars()
                        .take(24)
                        .collect(),
                    starred: slot.sources.iter().any(|source| source != "基础表"),
                    corner: slot.hint.as_ref().map(|hint| match hint {
                        Hint::Shorter(code) => format!("‹{code}›"),
                        Hint::Rest(rest) => rest.clone(),
                    }),
                    tooltip: format!(
                        "来源：{} · 编码 {} · {position}\n右键可置顶、调序、屏蔽",
                        slot.sources.join("、"),
                        slot.code
                    ),
                }
            })
            .collect();
        let preedit = self.preedit.clone();
        let empty = self.layout.is_empty();
        let look = Look {
            vertical: self.settings.vertical,
            scale: f32::from(self.settings.font_percent) / 100.0,
        };
        let highlight = self.highlight.saturating_sub(page * page_size);
        let position = self.window_position(ctx, anchor);
        let rows_width = self.rows_width().unwrap_or(0.0);
        let mut picked = None;
        let mut menu_open = false;
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
                        // 两行：上面编码串与页码，下面候选。挤在一行时编码、候选、页码
                        // 混作一团，读的人分不清哪个是打的、哪个是选的。
                        //
                        // 两行之间不画分隔线：一条线加上下留白要吃掉近 10px，而字号
                        // （12/14）与颜色（弱化/正文）已经把两行分得很开了。
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                            header(ui, &preedit, empty, page, pages, rows_width, look);
                            ui.add_space(ROW_GAP);
                            measured = candidates_row(
                                ui,
                                &rows,
                                highlight,
                                &mut picked,
                                &mut menu_open,
                                look,
                            );
                        });
                    });
            });
        self.remember_window(area.response.rect.size());
        self.remember_rows_width(measured);
        if let Some((index, pick)) = picked {
            self.apply_pick(ctx, page * page_size + index, index, pick);
        }
        // 点候选、开右键菜单都不该把编辑框的焦点带走：焦点一换，正在打的码就丢了。
        if (picked.is_some() || menu_open)
            && let Some(id) = self.focus_id
        {
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
    }

    /// 候选上的一次操作。`absolute` 是在整个布局里的位置，`page_index` 是本页第几个。
    fn apply_pick(&mut self, ctx: &egui::Context, absolute: usize, page_index: usize, pick: Pick) {
        if pick == Pick::Commit {
            self.commit_clicked(page_index);
            return;
        }
        let Some(slot) = self.layout.get(absolute).cloned() else {
            return;
        };
        let result = match pick {
            Pick::Top => self.move_word(&slot.code, &slot.text, 0),
            Pick::Up => self.move_word(&slot.code, &slot.text, -1),
            Pick::Down => self.move_word(&slot.code, &slot.text, 1),
            Pick::Block => self.block(&slot.code, &slot.text),
            Pick::Lookup => {
                ctx.data_mut(|data| data.insert_temp(egui::Id::new("ime-lookup"), slot.text));
                return;
            }
            Pick::Commit => unreachable!(),
        };
        self.notice = Some(match (result, pick) {
            (Ok(()), Pick::Block) => {
                format!("已屏蔽「{}」（{}），可在词表页恢复。", slot.text, slot.code)
            }
            (Ok(()), _) => format!("已调整「{}」在 {} 中的顺序。", slot.text, slot.code),
            (Err(error), _) => format!("操作失败：{error}"),
        });
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
            self.remember_commit(&text);
            self.pending_commit = Some(text);
            self.refresh(true);
        }
    }
}

/// 表头：左边编码串（空码时跟一个「空码」），右边页码（只有一页就不画）。
fn header(
    ui: &mut egui::Ui,
    preedit: &Preedit,
    empty: bool,
    page: usize,
    pages: usize,
    rows_width: f32,
    look: Look,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        code_strip(ui, preedit, look);
        if empty {
            ui.add_space(EMPTY_GAP);
            ui.label(
                egui::RichText::new("空码")
                    .size(look.small())
                    .color(theme::warn()),
            );
        }
        if pages <= 1 {
            return;
        }
        let text = format!("{}/{}", page + 1, pages);
        let galley = ui.painter().layout_no_wrap(
            text.clone(),
            egui::FontId::proportional(look.small()),
            theme::text_muted(),
        );
        // 页码顶到右边：按上一帧量到的候选行宽度算空档。候选行的宽度不受表头影响，
        // 两者不会互相牵扯；候选变窄时页码最多晚一帧跟上来。
        let space = rows_width - ui.min_rect().width() - galley.size().x;
        ui.add_space(space.max(HEADER_GAP));
        ui.label(
            egui::RichText::new(text)
                .size(look.small())
                .color(theme::text_muted()),
        );
    });
}

/// 候选行（竖排时是一列）。返回量到的宽度，下一帧的表头拿它摆页码。
///
/// 竖排时每个候选按自己的宽度左对齐，不撑满整列：Area 里用两端对齐的布局，
/// 可用宽度是整个视口，候选窗会被一下撑到屏幕那么宽。
fn candidates_row(
    ui: &mut egui::Ui,
    rows: &[Row],
    highlight: usize,
    picked: &mut Option<(usize, Pick)>,
    menu_open: &mut bool,
    look: Look,
) -> f32 {
    let mut add_rows = |ui: &mut egui::Ui| {
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
        for row in rows {
            let response =
                candidate_button(ui, row, row.index == highlight, look).on_hover_text(&row.tooltip);
            if response.clicked() {
                *picked = Some((row.index, Pick::Commit));
            }
            let menu = response.context_menu(|ui| {
                let items = [
                    ("置顶", Pick::Top),
                    ("上移", Pick::Up),
                    ("下移", Pick::Down),
                    ("屏蔽", Pick::Block),
                    ("查编码", Pick::Lookup),
                ];
                for (label, pick) in items {
                    if ui.button(label).clicked() {
                        *picked = Some((row.index, pick));
                        ui.close();
                    }
                }
            });
            *menu_open |= menu.is_some();
        }
    };
    let response = if look.vertical {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = CELL_GAP;
            add_rows(ui);
        })
        .response
    } else {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CELL_GAP;
            add_rows(ui);
        })
        .response
    };
    response.rect.width()
}

/// 编码串：光标位置用一条竖线标出来。字号比候选小一档、颜色弱一档，
/// 一眼就分得清哪一行是打进去的、哪一行是要选的。
fn code_strip(ui: &mut egui::Ui, preedit: &Preedit, look: Look) {
    let chars: Vec<char> = preedit.text.chars().collect();
    let caret = preedit.caret.min(chars.len());
    let before: String = chars[..caret].iter().collect();
    let after: String = chars[caret..].iter().collect();
    let small = |text: String, color: egui::Color32| {
        egui::RichText::new(text).size(look.small()).color(color)
    };
    ui.label(small(before, theme::text_muted()));
    ui.label(small("|".into(), theme::accent()));
    ui.label(small(after, theme::text_muted()));
}

/// 一个候选：弱化的小号序号 + 正文字号的候选文本；不只在基础表里的词（公文词表、
/// 导入表、个人词条）在词后右上角带星标，简码与逐码提示的小字跟在最后。
fn candidate_button(ui: &mut egui::Ui, row: &Row, selected: bool, look: Look) -> egui::Response {
    let format = |size: f32, color: egui::Color32| egui::TextFormat {
        font_id: egui::FontId::proportional(size),
        color,
        ..Default::default()
    };
    // 选中的序号跟着高亮走，但要比候选本身淡：它只是个按键提示。
    let hint_color = if selected {
        theme::accent_active().gamma_multiply(0.7)
    } else {
        theme::text_muted()
    };
    let index = (row.index + 1).to_string();
    let mut job = egui::text::LayoutJob::default();
    job.append(&index, 0.0, format(look.small(), hint_color));
    job.append(
        &row.text,
        INDEX_GAP,
        format(
            look.body(),
            if selected {
                theme::accent_active()
            } else {
                theme::text()
            },
        ),
    );
    if row.starred {
        // 给星标让出位置：一个几乎没宽度的空格，靠前导空白撑开
        job.append(
            " ",
            SPARKLE_GAP + look.sparkle(),
            format(1.0, egui::Color32::TRANSPARENT),
        );
    }
    // 简码 / 逐码小字：序号那档字号、弱化色，顶到行首当上标
    let corner_format = egui::TextFormat {
        valign: egui::Align::TOP,
        ..format(look.small(), theme::text_muted())
    };
    let corner_width = row.corner.as_ref().map_or(0.0, |corner| {
        job.append(corner, CORNER_GAP, corner_format.clone());
        CORNER_GAP
            + ui.painter()
                .layout_no_wrap(
                    corner.clone(),
                    corner_format.font_id.clone(),
                    corner_format.color,
                )
                .size()
                .x
    });
    let button = egui::Button::new(job)
        .stroke(egui::Stroke::NONE)
        .corner_radius(4);
    let response = ui.add(if selected {
        button.fill(theme::accent_soft())
    } else {
        button
    });
    if row.starred {
        // 贴着候选词右上角：右边收进内边距，顶上与字形顶部大致齐平
        let content = response.rect.shrink2(CELL_PADDING);
        let min = egui::pos2(
            content.right() - corner_width - look.sparkle(),
            content.top() + content.height() * 0.15,
        );
        let color = if selected {
            theme::accent_active()
        } else {
            theme::accent()
        };
        ui.painter().add(sparkle(
            egui::Rect::from_min_size(min, egui::Vec2::splat(look.sparkle())),
            color,
        ));
    }
    response
}

/// 四角星：四个尖在方块各边中点，相邻两尖之间是一条向中心弯的二次曲线。
/// 星形对中心是「星形域」，从中心扇形三角化就能实心填满（epaint 的多边形填充只认凸形）。
fn sparkle(rect: egui::Rect, color: egui::Color32) -> egui::Shape {
    /// 每段曲线切成几截。
    const STEPS: usize = 4;
    let c = rect.center();
    let r = rect.width() / 2.0;
    let w = r * SPARKLE_WAIST;
    let tips = [
        egui::vec2(0.0, -r),
        egui::vec2(r, 0.0),
        egui::vec2(0.0, r),
        egui::vec2(-r, 0.0),
    ];
    let controls = [
        egui::vec2(w, -w),
        egui::vec2(w, w),
        egui::vec2(-w, w),
        egui::vec2(-w, -w),
    ];
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, color);
    for (i, (&from, &control)) in tips.iter().zip(&controls).enumerate() {
        let to = tips[(i + 1) % tips.len()];
        for step in 0..STEPS {
            let t = step as f32 / STEPS as f32;
            let u = 1.0 - t;
            let point = from * (u * u) + control * (2.0 * u * t) + to * (t * t);
            mesh.colored_vertex(c + point, color);
        }
    }
    let outline = (tips.len() * STEPS) as u32;
    for i in 0..outline {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % outline);
    }
    egui::Shape::mesh(mesh)
}

#[cfg(test)]
mod tests {
    use super::super::{ImeSettings, table};
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    fn ime(code: &str) -> Ime {
        let mut ime = Ime::bare(ImeSettings::default());
        ime.table.base = table::parse("abcd,1=公文\nab,1=公文", false).entries;
        ime.table.personal.entries = table::parse("abcd,1=个人词", false).entries;
        ime.table.rebuild();
        ime.anchor = Some(egui::Rect::from_min_size(
            egui::pos2(20.0, 20.0),
            egui::vec2(2.0, 16.0),
        ));
        for c in code.chars() {
            ime.execute_guarded(Action::Push(c));
        }
        ime
    }

    #[test]
    fn window_shows_hints_and_empty_code() {
        let mut harness =
            Harness::new_ui_state(|ui, ime: &mut Ime| ime.candidates_ui(ui.ctx()), ime("abcd"));
        harness.run();
        // 简码提示跟在词后；个人词条带星标（画出来的，不在文字里）。
        harness.get_by_label_contains("‹ab›");
        harness.get_by_label_contains("个人词");
        let mut harness =
            Harness::new_ui_state(|ui, ime: &mut Ime| ime.candidates_ui(ui.ctx()), ime("zz"));
        harness.run();
        harness.get_by_label("空码");
    }
}
