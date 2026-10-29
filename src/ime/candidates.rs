//! 候选窗：贴光标画一行（或竖排一列）候选 + 编码串。
//!
//! 应用内输入法没有系统候选窗可用（系统输入法已经被关掉了），所以这一块自己画：
//! 编码串与它的光标位置、当前页的候选、页码。两码起表头右侧有个向下箭头，
//! 点开（或按向下、下一页）是可滚动的候选网格：每行与候选条一样多格，露四行。
//!
//! 中英模式不画在这里：状态栏已经常驻显示（见 `app::chrome`），候选窗再挂一个既重复，
//! 又把一个全局状态混进了这一次组字的信息里。
//!
//! 上屏走 [`Ime::pending_commit`]：点击发生在本帧事件处理之后，文本框这一帧已经画完了，
//! 只能把文本排到下一帧的事件队列最前面（见 `session::Ime::begin_frame`）。

use eframe::egui;

use super::browser::{Kind, VISIBLE_ROWS};
use super::session::{Hint, Ime, Preedit};
use crate::theme::{self, Icon};

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

/// 展开网格里行与行的间隙。
const GRID_ROW_GAP: f32 = 1.0;

/// 展开网格一格最多显示几个字，再长的截断加省略号（悬停看全词）。
const GRID_MAX_CHARS: usize = 5;

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
    /// 在整个布局里的位置。
    absolute: usize,
    /// 这一格的数字键（1 起）。
    number: usize,
    /// 画不画序号：展开网格只给高亮所在的那一行标号，别的行留出同样的宽度。
    numbered: bool,
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
        let page_size = self.candidate_page_size();
        let browsing = self.browser.open;
        let page = if browsing {
            0
        } else {
            self.highlight / page_size
        };
        // 能展开时，候选条上的下一页就是展开，不在表头标页码，免得看着像能翻。
        let can_browse = self.can_browse();
        let pages = if can_browse {
            1
        } else {
            // 页数按设置里的每页格数算；空布局也算一页。
            self.layout.len().div_ceil(page_size).max(1)
        };
        // 先把要画的东西抄成自己的数据：闭包里还要改 `self`（记下点中的候选）。
        // 展开网格的格子在滚动区里按可见行现抄，不在这里一次抄全。
        let rows: Vec<Row> = if browsing {
            Vec::new()
        } else {
            (page * page_size..self.layout.len().min((page + 1) * page_size))
                .map(|absolute| self.row(absolute, absolute % page_size + 1, true, usize::MAX))
                .collect()
        };
        let preedit = self.preedit.clone();
        let empty = self.layout.is_empty();
        let look = Look {
            vertical: self.settings.vertical,
            scale: f32::from(self.settings.font_percent) / 100.0,
        };
        let position = self.window_position(ctx, anchor);
        let rows_width = self.rows_width().unwrap_or(0.0);
        let scroll_to = self.browser.scroll_to.take();
        let mut top_row = self.browser.top_row;
        let mut picked = None;
        let mut menu_open = false;
        let mut measured = 0.0;
        let mut toggle = false;
        let mut kind = self.browser.kind;
        let mut filter_changed = false;
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
                            ui.horizontal(|ui| {
                                if browsing {
                                    code_strip(ui, &preedit, look);
                                    ui.add_space(HEADER_GAP);
                                    filter_changed = kind_filter(ui, &mut kind, look);
                                } else {
                                    header(ui, &preedit, empty, page, pages, rows_width, look);
                                }
                                if can_browse {
                                    // 箭头顶到右边：按上一帧量到的候选区宽度算空档。
                                    let space =
                                        rows_width - ui.min_rect().width() - chevron_width(look);
                                    ui.add_space(space.max(HEADER_GAP));
                                    toggle = if browsing {
                                        chevron(ui, Icon::ChevronUp, "收起候选", look)
                                    } else {
                                        chevron(ui, Icon::ChevronDown, "展开候选", look)
                                    }
                                    .clicked();
                                }
                            });
                            ui.add_space(ROW_GAP);
                            measured = if browsing {
                                browse_grid(
                                    ui,
                                    self,
                                    look,
                                    scroll_to,
                                    &mut top_row,
                                    &mut picked,
                                    &mut menu_open,
                                )
                            } else {
                                candidates_row(
                                    ui,
                                    &rows,
                                    self.highlight,
                                    &mut picked,
                                    &mut menu_open,
                                    look,
                                )
                            };
                        });
                    });
            });
        self.remember_window(area.response.rect.size());
        self.remember_rows_width(measured);
        if browsing {
            self.browser.top_row = top_row;
        }
        if toggle {
            if browsing {
                self.collapse_browser();
            } else {
                self.expand_browser();
            }
        } else if filter_changed {
            self.browser.kind = kind;
            self.filter_browser();
        }
        if let Some((absolute, pick)) = picked {
            self.apply_pick(ctx, absolute, pick);
        }
        // 点候选、开右键菜单都不该把编辑框的焦点带走：焦点一换，正在打的码就丢了。
        // 点在窗里的空白处（编码串、格间空隙、滚动条）同样会让编辑框交出焦点，一并要回来。
        let clicked_inside = ctx.input(|input| {
            input.pointer.any_click()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| area.response.rect.contains(pos))
        });
        if (picked.is_some() || menu_open || toggle || filter_changed || clicked_inside)
            && let Some(id) = self.focus_id
        {
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
    }

    /// 抄一格候选。`max_chars` 之外的字截掉换成省略号。
    fn row(&self, absolute: usize, number: usize, numbered: bool, max_chars: usize) -> Row {
        let slot = &self.layout[absolute];
        let position = if absolute < self.exact {
            format!("第 {} 位", absolute + 1)
        } else {
            "逐码提示".to_string()
        };
        let text: Vec<char> = slot
            .text
            .replace(['\n', '\r'], " ")
            .chars()
            .take(24)
            .collect();
        let text = if text.len() > max_chars {
            let mut short: String = text[..max_chars.saturating_sub(1)].iter().collect();
            short.push('…');
            short
        } else {
            text.into_iter().collect()
        };
        Row {
            absolute,
            number,
            numbered,
            text,
            starred: slot.sources.iter().any(|source| source != "基础表"),
            corner: slot.hint.as_ref().map(corner_text),
            tooltip: format!(
                "{} · 来源：{} · 编码 {} · {position}\n右键可置顶、调序、屏蔽",
                slot.text,
                slot.sources.join("、"),
                slot.code
            ),
        }
    }

    /// 候选上的一次操作。`absolute` 是在整个布局里的位置。
    fn apply_pick(&mut self, ctx: &egui::Context, absolute: usize, pick: Pick) {
        if pick == Pick::Commit {
            self.commit_clicked(absolute);
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
            // 提交在上面已经处理过；真走到这里也不该崩，按「无动作」收场。
            Pick::Commit => return,
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
    fn commit_clicked(&mut self, absolute: usize) {
        if let Some(text) = self.commit_at(absolute) {
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

/// 候选块的底色与描边：没选中的不描边、不上底，只在鼠标底下才浮出一块淡底。
fn cell_visuals(ui: &mut egui::Ui) {
    ui.spacing_mut().button_padding = CELL_PADDING;
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    widgets.hovered.weak_bg_fill = theme::surface_hover();
    widgets.active.weak_bg_fill = theme::surface_active();
    widgets.hovered.bg_stroke = egui::Stroke::NONE;
    widgets.active.bg_stroke = egui::Stroke::NONE;
    // 悬停时不让候选块胀大：一胀整行就跟着抖。
    widgets.hovered.expansion = 0.0;
    widgets.active.expansion = 0.0;
}

/// 画一格候选并接住左键上屏与右键菜单。
fn candidate_cell(
    ui: &mut egui::Ui,
    row: &Row,
    selected: bool,
    picked: &mut Option<(usize, Pick)>,
    menu_open: &mut bool,
    look: Look,
    small: bool,
) {
    let response = candidate_button(ui, row, selected, look, small).on_hover_text(&row.tooltip);
    if response.clicked() {
        *picked = Some((row.absolute, Pick::Commit));
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
                *picked = Some((row.absolute, pick));
                ui.close();
            }
        }
    });
    *menu_open |= menu.is_some();
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
        cell_visuals(ui);
        for row in rows {
            candidate_cell(
                ui,
                row,
                row.absolute == highlight,
                picked,
                menu_open,
                look,
                false,
            );
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

/// 展开后的候选网格：每行与候选条一样多格、等宽，露 [`VISIBLE_ROWS`] 行，其余滚动。
/// 只画看得见的行，几百条候选也不拖慢。`scroll_to` 是按键要求滚到的顶行，
/// 画完把实际顶行（可能被鼠标滚过）写回 `top_row`。返回网格宽度。
fn browse_grid(
    ui: &mut egui::Ui,
    ime: &Ime,
    look: Look,
    scroll_to: Option<usize>,
    top_row: &mut usize,
    picked: &mut Option<(usize, Pick)>,
    menu_open: &mut bool,
) -> f32 {
    let cols = ime.candidate_page_size();
    let total = ime.layout.len();
    if total == 0 {
        return ui
            .label(
                egui::RichText::new("没有符合的候选")
                    .size(look.small())
                    .color(theme::text_muted()),
            )
            .rect
            .width();
    }
    let (cell, row_height, max_chars) = grid_metrics(ui, ime, look);
    let stride = row_height + GRID_ROW_GAP;
    let bar = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
    let highlight_row = ime.highlight / cols;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("gw-ime-browse")
        .max_height(stride * VISIBLE_ROWS as f32 - GRID_ROW_GAP)
        .auto_shrink([true, true]);
    if let Some(top) = scroll_to {
        scroll = scroll.vertical_scroll_offset(top as f32 * stride);
    }
    let output = scroll.show_rows(ui, row_height, total.div_ceil(cols), |ui, range| {
        ui.spacing_mut().item_spacing = egui::vec2(CELL_GAP, GRID_ROW_GAP);
        cell_visuals(ui);
        for line in range {
            ui.horizontal(|ui| {
                for col in 0..cols {
                    let absolute = line * cols + col;
                    let size = egui::vec2(cell, row_height);
                    ui.allocate_ui_with_layout(
                        size,
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_size(size);
                            if absolute < total {
                                let row =
                                    ime.row(absolute, col + 1, line == highlight_row, max_chars);
                                let selected = absolute == ime.highlight;
                                candidate_cell(ui, &row, selected, picked, menu_open, look, true);
                            }
                        },
                    );
                }
                // 给悬浮滚动条让出位置，免得压住最后一列。
                ui.add_space(bar);
            });
        }
    });
    *top_row = (output.state.offset.y / stride).round() as usize;
    // 宽度按格子算，不取滚动区量到的：表头按这个宽度摆箭头，取量到的会互相撑大。
    cols as f32 * cell + (cols - 1) as f32 * CELL_GAP + bar
}

/// 网格一格的宽高与最多显示几个字：按全部候选里最长的词与最长的词后小字量，
/// 一屏之内各列等宽，滚动时不跳。
fn grid_metrics(ui: &egui::Ui, ime: &Ime, look: Look) -> (f32, f32, usize) {
    let chars = ime
        .layout
        .iter()
        .map(|slot| slot.text.chars().count())
        .max()
        .unwrap_or(1)
        .clamp(1, GRID_MAX_CHARS);
    let corner = ime
        .layout
        .iter()
        .filter_map(|slot| {
            slot.hint
                .as_ref()
                .map(|hint| corner_text(hint).chars().count())
        })
        .max()
        .unwrap_or(0);
    let starred = ime
        .layout
        .iter()
        .any(|slot| slot.sources.iter().any(|source| source != "基础表"));
    let measure = |text: String, size: f32| {
        ui.painter()
            .layout_no_wrap(text, egui::FontId::proportional(size), theme::text())
            .size()
    };
    let body = measure("国".repeat(chars), look.body());
    let mut width = measure("8".into(), look.small()).x + INDEX_GAP + body.x;
    if starred {
        width += SPARKLE_GAP + look.sparkle();
    }
    if corner > 0 {
        width += CORNER_GAP + measure("m".repeat(corner), look.small()).x;
    }
    let width = (width + CELL_PADDING.x * 2.0 + 2.0).ceil();
    let height = (body.y + CELL_PADDING.y * 2.0).ceil();
    (width, height, chars)
}

/// 词后小字：更短的编码带尖括号，逐码提示只写还要打的码。
fn corner_text(hint: &Hint) -> String {
    match hint {
        Hint::Shorter(code) => format!("‹{code}›"),
        Hint::Rest(rest) => rest.clone(),
    }
}

/// 展开后表头的「全部 / 单字 / 词组」。返回这一帧有没有切换。
fn kind_filter(ui: &mut egui::Ui, kind: &mut Kind, look: Look) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = CELL_GAP;
    ui.spacing_mut().button_padding = egui::vec2(6.0, 0.0);
    for (value, label) in [
        (Kind::All, "全部"),
        (Kind::Character, "单字"),
        (Kind::Phrase, "词组"),
    ] {
        changed |= ui
            .selectable_value(kind, value, egui::RichText::new(label).size(look.small()))
            .changed();
    }
    changed
}

/// 表头右侧的展开 / 收起箭头：只有图形，`label` 作悬停说明与无障碍名称。
fn chevron(ui: &mut egui::Ui, icon: Icon, label: &str, look: Look) -> egui::Response {
    ui.spacing_mut().button_padding = egui::vec2(2.0, 0.0);
    let response = ui.add(
        egui::Button::image(icon.image_sized(chevron_size(look)))
            .image_tint_follows_text_color(true)
            .frame(false)
            .small(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response.on_hover_text(label)
}

fn chevron_size(look: Look) -> f32 {
    look.small() + 2.0
}

fn chevron_width(look: Look) -> f32 {
    chevron_size(look) + 4.0
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
fn candidate_button(
    ui: &mut egui::Ui,
    row: &Row,
    selected: bool,
    look: Look,
    small: bool,
) -> egui::Response {
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
    let index = row.number.to_string();
    let mut job = egui::text::LayoutJob::default();
    let hint_color = if row.numbered {
        hint_color
    } else {
        egui::Color32::TRANSPARENT
    };
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
    // 展开网格用小号按钮：不受全局控件行高托底，四行才排得紧。
    let button = if small { button.small() } else { button };
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
    use super::super::{ImeSettings, keys::Action, keys::Navigation, table};
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
    #[test]
    fn expanded_panel_filters_and_commits_a_visible_candidate() {
        let mut harness =
            Harness::new_ui_state(|ui, ime: &mut Ime| ime.candidates_ui(ui.ctx()), ime("ab"));
        harness.run();
        harness.get_by_label("展开候选").click();
        harness.run();
        assert!(harness.state().browser.open);
        harness.get_by_label("单字").click();
        harness.run();
        assert!(harness.state().layout.is_empty());
        harness.get_by_label("词组").click();
        harness.run();
        assert_eq!(harness.state().layout.len(), 2);
        harness.get_by_label_contains("个人词").click();
        harness.run();
        assert_eq!(harness.state().pending_commit.as_deref(), Some("个人词"));
    }

    #[test]
    fn single_code_has_no_expand_button() {
        let mut harness =
            Harness::new_ui_state(|ui, ime: &mut Ime| ime.candidates_ui(ui.ctx()), ime("a"));
        harness.run();
        assert!(harness.query_by_label("展开候选").is_none());
    }

    #[test]
    fn clicking_blank_panel_space_keeps_editor_focus() {
        let editor = egui::Id::new("editor");
        let mut harness = Harness::new_ui_state(
            |ui, (ime, text, started): &mut (Ime, String, bool)| {
                ui.add(egui::TextEdit::singleline(text).id(editor));
                if !*started {
                    ui.memory_mut(|memory| memory.request_focus(editor));
                    *started = true;
                }
                ime.candidates_ui(ui.ctx());
            },
            (
                {
                    let mut ime = ime("ab");
                    ime.focus_id = Some(editor);
                    ime
                },
                String::new(),
                false,
            ),
        );
        harness.run();
        harness.get_by_label("展开候选").click();
        harness.run();
        // 编码串里的光标竖线是不可点的标签，点它等于点在窗里的空白处。
        harness.get_by_label("|").click();
        harness.run();
        assert!(harness.ctx.memory(|memory| memory.has_focus(editor)));
    }

    #[test]
    fn grid_shows_four_rows_scrolls_with_keys_and_commits_the_clicked_cell() {
        let mut text = String::from("ab,1=简码\n");
        for i in 0..40u8 {
            text.push_str(&format!(
                "ab{}{},1=词{i}\n",
                char::from(b'a' + i / 26),
                char::from(b'a' + i % 26)
            ));
        }
        let mut state = Ime::bare(ImeSettings::default());
        state.table.base = table::parse(&text, false).entries;
        state.table.rebuild();
        state.anchor = Some(egui::Rect::from_min_size(
            egui::pos2(20.0, 20.0),
            egui::vec2(2.0, 16.0),
        ));
        for c in "ab".chars() {
            state.execute_guarded(Action::Push(c));
        }
        state.expand_browser();
        let mut harness =
            Harness::new_ui_state(|ui, ime: &mut Ime| ime.candidates_ui(ui.ctx()), state);
        harness.run();
        // 每行 5 格、露 4 行。第 5 行（词19 起）滚动区备着画了，但被裁在可见区外，点不中。
        // 窗宽只够 5 列：曾经表头按量到的网格宽摆箭头，两边互相撑到五百多像素。
        let width = harness.state().window_size().unwrap().x;
        assert!(width < 520.0, "候选网格被撑宽到 {width}");
        harness.get_by_label_contains("词22").click();
        harness.run();
        assert_eq!(harness.state().pending_commit, None);
        // 翻页把高亮挪到第 5 行（第 21 格），网格滚一行露出它。
        harness
            .state_mut()
            .execute_guarded(Action::Navigate(Navigation::Page(1)));
        harness.run();
        harness.run();
        assert_eq!(harness.state().browser.top_row, 1);
        // 点的是第 5 行，不是高亮那格：上屏按点中的位置算。
        harness.get_by_label_contains("词22").click();
        harness.run();
        assert_eq!(harness.state().pending_commit.as_deref(), Some("词22"));
    }
}
