//! 候选窗：贴光标画一行（或竖排一列）候选 + 编码串。
//!
//! 应用内输入法没有系统候选窗可用（系统输入法已经被关掉了），所以这一块自己画：
//! 编码串与它的光标位置、当前页的候选、页码。两码起表头右侧有个向下箭头，
//! 点开（或按向下、下一页）是只列单字的找字网格：每行与候选条一样多格，露四行，
//! 还要打的码放在字右上方的独立标签里，行线分隔各行。
//!
//! 中英模式不画在这里：状态栏已经常驻显示（见 `app::chrome`），候选窗再挂一个既重复，
//! 又把一个全局状态混进了这一次组字的信息里。
//!
//! 上屏走 [`Ime::pending_commit`]：点击发生在本帧事件处理之后，文本框这一帧已经画完了，
//! 只能把文本排到下一帧的事件队列最前面（见 `session::Ime::begin_frame`）。

use eframe::egui;

use super::browser::VISIBLE_ROWS;
use super::session::{Hint, Ime, Preedit};
use crate::theme::{self, Icon};

/// 测量与绘制使用同一字体族，避免换字体后候选格尺寸失配。
fn ime_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(theme::IME_FONT_FAMILY.into()))
}

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

/// 词后右上角小字（简码、逐码提示）与前面候选词的间距。
const CORNER_GAP: f32 = 1.0;

/// 展开网格里行与行的间隙。
const GRID_ROW_GAP: f32 = 6.0;

/// 候选窗的排法与字号，设置页里来。字号按比例放大正文与小字两档，
/// 候选文字的合成加粗跟着放大，间距不动。
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
}

/// 候选窗中的序号、文字、业务词加粗与词后小字。
struct Row {
    /// 在整个布局里的位置。
    absolute: usize,
    /// 这一格的数字键（1 起）。
    number: usize,
    /// 画不画序号：展开网格只给高亮所在的那一行标号，别的行留出同样的宽度。
    numbered: bool,
    text: String,
    /// 来自公文词表、导入表或个人词条（不只在基础表里）。
    emphasized: bool,
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
                .map(|absolute| self.row(absolute, absolute % page_size + 1, true))
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
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                            if browsing {
                                // 展开面板的宽度由网格决定，表头与底栏不能反过来撑宽它。
                                ui.spacing_mut().scroll.bar_width = 4.0;
                                ui.spacing_mut().scroll.bar_inner_margin = 4.0;
                                let cell = grid_cell_size(ui, self, look);
                                let width = grid_width(ui, self.candidate_page_size(), cell);
                                toggle = browser_header(ui, &preedit, width, look);
                                grid_rule(ui, width, look);
                                measured = browse_grid(
                                    ui,
                                    self,
                                    look,
                                    scroll_to,
                                    &mut top_row,
                                    &mut picked,
                                    &mut menu_open,
                                );
                                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                                grid_rule(ui, width, look);
                                browser_footer(ui, self.layout.len(), width, look);
                                return;
                            }
                            ui.horizontal(|ui| {
                                if browsing {
                                    code_strip(ui, &preedit, look);
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
                            measured = candidates_row(
                                ui,
                                &rows,
                                self.highlight,
                                &mut picked,
                                &mut menu_open,
                                look,
                            );
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
        if (picked.is_some() || menu_open || toggle || clicked_inside)
            && let Some(id) = self.focus_id
        {
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
    }

    /// 抄一格候选。
    fn row(&self, absolute: usize, number: usize, numbered: bool) -> Row {
        let slot = &self.layout[absolute];
        let position = if absolute < self.exact {
            format!("第 {} 位", absolute + 1)
        } else {
            "逐码提示".to_string()
        };
        let text = slot
            .text
            .replace(['\n', '\r'], " ")
            .chars()
            .take(24)
            .collect();
        Row {
            absolute,
            number,
            numbered,
            text,
            emphasized: slot.sources.iter().any(|source| source != "基础表"),
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
                    .family(egui::FontFamily::Name(theme::IME_FONT_FAMILY.into()))
                    .size(look.small())
                    .color(theme::warn()),
            );
        }
        if pages <= 1 {
            return;
        }
        let text = format!("{}/{}", page + 1, pages);
        let galley =
            ui.painter()
                .layout_no_wrap(text.clone(), ime_font(look.small()), theme::text_muted());
        // 页码顶到右边：按上一帧量到的候选行宽度算空档。候选行的宽度不受表头影响，
        // 两者不会互相牵扯；候选变窄时页码最多晚一帧跟上来。
        let space = rows_width - ui.min_rect().width() - galley.size().x;
        ui.add_space(space.max(HEADER_GAP));
        ui.label(
            egui::RichText::new(text)
                .family(egui::FontFamily::Name(theme::IME_FONT_FAMILY.into()))
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
) {
    let response = candidate_button(ui, row, selected, look);
    pick_actions(response, row, picked, menu_open);
}

/// 候选格上的左键上屏与右键菜单（置顶、调序、屏蔽、查码）。
fn pick_actions(
    response: egui::Response,
    row: &Row,
    picked: &mut Option<(usize, Pick)>,
    menu_open: &mut bool,
) {
    let response = response.on_hover_text(&row.tooltip);
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
            candidate_cell(ui, row, row.absolute == highlight, picked, menu_open, look);
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

/// 展开后的找字网格：只有单字，每行与候选条一样多格、等宽，露 [`VISIBLE_ROWS`] 行，
/// 其余滚动。只画看得见的行，几百个字也不拖慢。`scroll_to` 是按键要求滚到的顶行，
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
                egui::RichText::new("没有可选的单字")
                    .family(egui::FontFamily::Name(theme::IME_FONT_FAMILY.into()))
                    .size(look.small())
                    .color(theme::text_muted()),
            )
            .rect
            .width();
    }
    let cell = grid_cell_size(ui, ime, look);
    // 行高不等于格高：egui 的横向行带至少 `interact_size.y`（本主题 30 px）。候选字体
    // 比界面字体小、或本机缺中文字体回退到默认字面时，一格（27 px）比行带矮，按格高算
    // 高度就会把第 4 行裁掉一截，滚动的行距也会跟着错位。
    let row_height = cell.y.max(ui.spacing().interact_size.y);
    let stride = row_height + GRID_ROW_GAP;
    let highlight_row = ime.highlight / cols;
    // 高度钉死在可见的那几行：只给上限的话
    // 滚动区会按自己的最小高度缩，实测只露出两行多。
    let lines = total.div_ceil(cols);
    let height = stride * lines.min(VISIBLE_ROWS) as f32 - GRID_ROW_GAP;
    // 行距先设在滚动区的外层 ui 上：`show_rows` 按调用时的 `item_spacing` 排行距，
    // 若在闭包里才设，它会按旧行距摆行，与这里算的 `stride` 对不上。
    ui.spacing_mut().item_spacing = egui::vec2(CELL_GAP, GRID_ROW_GAP);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("gw-ime-browse")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([true, true]);
    if let Some(top) = scroll_to {
        scroll = scroll.vertical_scroll_offset(top as f32 * stride);
    }
    let output = scroll.show_rows(ui, row_height, lines, |ui, range| {
        ui.spacing_mut().item_spacing = egui::vec2(CELL_GAP, GRID_ROW_GAP);
        for line in range {
            let row_response = ui.horizontal(|ui| {
                for col in 0..cols {
                    let absolute = line * cols + col;
                    if absolute >= total {
                        break;
                    }
                    let row = ime.row(absolute, col + 1, line == highlight_row);
                    let response = grid_cell(ui, &row, absolute == ime.highlight, look, cell);
                    pick_actions(response, &row, picked, menu_open);
                }
            });
            if line + 1 < lines {
                let y = row_response.response.rect.bottom() + GRID_ROW_GAP * 0.5;
                ui.painter().hline(
                    row_response.response.rect.x_range(),
                    y,
                    egui::Stroke::new(1.0, theme::border().gamma_multiply(0.6)),
                );
            }
        }
    });
    *top_row = (output.state.offset.y / stride).round() as usize;
    // 宽度按格子算，不取滚动区量到的：表头按这个宽度摆箭头，取量到的会互相撑大。
    grid_width(ui, cols, cell)
}

/// 网格总宽固定，包含滚动条位置，出现滚动条时列宽不跳。
fn grid_width(ui: &egui::Ui, cols: usize, cell: egui::Vec2) -> f32 {
    cols as f32 * cell.x
        + (cols - 1) as f32 * CELL_GAP
        + ui.spacing().scroll.bar_width
        + ui.spacing().scroll.bar_inner_margin
}

/// 展开面板的表头：编码、区域名称与收起按钮。
fn browser_header(ui: &mut egui::Ui, preedit: &Preedit, width: f32, look: Look) -> bool {
    let mut collapse = false;
    ui.allocate_ui_with_layout(
        egui::vec2(width, look.small() + 6.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            code_strip(ui, preedit, look);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                collapse = chevron(ui, Icon::ChevronUp, "收起候选", look).clicked();
                if width > 170.0 * look.scale {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("单字候选")
                            .font(ime_font(look.small()))
                            .color(theme::text_muted()),
                    );
                }
            });
        },
    );
    collapse
}

/// 分区线留出呼吸空间，网格内部只画更淡的行线。
fn grid_rule(ui: &mut egui::Ui, width: f32, look: Look) {
    ui.add_space(5.0 * look.scale);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, theme::border()),
    );
    ui.add_space(5.0 * look.scale);
}

/// 底栏不参与列宽计算；窄网格只显示数量，留足位置才显示操作提示。
fn browser_footer(ui: &mut egui::Ui, total: usize, width: f32, look: Look) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, look.small() + 4.0 * look.scale),
        egui::Sense::hover(),
    );
    let count = format!("{total} 个候选");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &count));
    let painter = ui.painter();
    let font = ime_font(look.small() * 0.9);
    let count = painter.layout_no_wrap(count, font.clone(), theme::text_muted());
    painter.galley(
        egui::pos2(rect.left(), rect.center().y - count.size().y * 0.5),
        count.clone(),
        theme::text_muted(),
    );
    let hint = painter.layout_no_wrap("↑↓ 选择 · 空格上屏".into(), font, theme::text_muted());
    if count.size().x + hint.size().x + 16.0 * look.scale <= width {
        painter.galley(
            egui::pos2(
                rect.right() - hint.size().x,
                rect.center().y - hint.size().y * 0.5,
            ),
            hint,
            theme::text_muted(),
        );
    }
}

/// 网格留出独立的序号、汉字与辅码标签三块位置；按最长辅码量宽，滚动时不跳。
fn grid_cell_size(ui: &egui::Ui, ime: &Ime, look: Look) -> egui::Vec2 {
    let corner = ime
        .layout
        .iter()
        .filter_map(|slot| {
            slot.hint
                .as_ref()
                .map(|hint| corner_text(hint).chars().count())
        })
        .max()
        .unwrap_or(0)
        .max(1);
    let measure = |text: String, size: f32| {
        ui.painter()
            .layout_no_wrap(text, ime_font(size), theme::text())
            .size()
    };
    let body = measure("国".into(), look.body() * 1.2);
    let code = measure("m".repeat(corner), look.small() * 0.85);
    let number = measure("8".into(), look.small() * 0.85);
    egui::vec2(
        (number.x + body.x + code.x + 22.0 * look.scale).ceil(),
        (body.y.max(code.y) + 18.0 * look.scale).ceil(),
    )
}

/// 汉字是阅读锚点，辅码单独放在右上角标签里；序号在左上，业务词仅加粗汉字。
fn grid_cell(
    ui: &mut egui::Ui,
    row: &Row,
    selected: bool,
    look: Look,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, row.text.clone())
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let fill = if selected {
        theme::accent_soft()
    } else if response.hovered() {
        theme::surface_hover()
    } else {
        egui::Color32::TRANSPARENT
    };
    painter.rect_filled(rect.shrink(1.0), 5.0, fill);
    let text_color = if selected {
        theme::accent_active()
    } else {
        theme::text()
    };
    let muted = if selected {
        theme::accent_active()
    } else {
        theme::text_muted()
    };
    let small = ime_font(look.small() * 0.85);
    let inner = rect.shrink2(egui::vec2(4.0, 4.0) * look.scale);
    let number_width = painter
        .layout_no_wrap("8".into(), small.clone(), muted)
        .size()
        .x;
    let body = painter.layout_no_wrap(row.text.clone(), ime_font(look.body() * 1.2), text_color);
    // 字的横向位置不随辅码长短变化；码和字之间始终留出独立的空隙。
    let body_pos = egui::pos2(
        inner.left() + number_width + 3.0 * look.scale,
        inner.center().y - body.size().y * 0.5 + 2.0 * look.scale,
    );
    painter.galley(body_pos, body.clone(), text_color);
    if let Some(corner) = &row.corner {
        let code = painter.layout_no_wrap(corner.clone(), small.clone(), muted);
        let badge = egui::Rect::from_min_size(
            egui::pos2(body_pos.x + body.size().x + 3.0 * look.scale, inner.top()),
            code.size() + egui::vec2(5.0, 3.0) * look.scale,
        );
        let fill = if selected {
            theme::accent_active().gamma_multiply(0.12)
        } else {
            theme::text_muted().gamma_multiply(0.10)
        };
        painter.rect_filled(badge, 3.0, fill);
        painter.galley(badge.center() - code.size() * 0.5, code, muted);
    }
    if row.numbered {
        painter.text(
            inner.left_top(),
            egui::Align2::LEFT_TOP,
            row.number.to_string(),
            small,
            muted,
        );
    }
    if row.emphasized {
        paint_bold_copy(painter, body_pos, &body, text_color, look);
    }
    response
}

/// 词后小字：更短的编码带尖括号，逐码提示只写还要打的码。
fn corner_text(hint: &Hint) -> String {
    match hint {
        Hint::Shorter(code) => format!("‹{code}›"),
        Hint::Rest(rest) => rest.clone(),
    }
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
        egui::RichText::new(text)
            .font(ime_font(look.small()))
            .color(color)
    };
    ui.label(small(before, theme::text_muted()));
    ui.label(small("|".into(), theme::accent()));
    ui.label(small(after, theme::text_muted()));
}

/// 一个候选：弱化的小号序号 + 正文字号的候选文本；不只在基础表里的词（公文词表、
/// 导入表、个人词条）仅加粗候选文字，简码与逐码提示的小字跟在最后。
fn candidate_button(ui: &mut egui::Ui, row: &Row, selected: bool, look: Look) -> egui::Response {
    let format = |size: f32, color: egui::Color32| egui::TextFormat {
        font_id: ime_font(size),
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
    // 简码 / 逐码小字：序号那档字号、弱化色，顶到行首当上标
    let corner_format = egui::TextFormat {
        valign: egui::Align::TOP,
        ..format(look.small(), theme::text_muted())
    };
    if let Some(corner) = &row.corner {
        job.append(corner, CORNER_GAP, corner_format);
    }
    let label = job.text.clone();
    let galley = ui.painter().layout_job(job);
    let size =
        (galley.size() + CELL_PADDING * 2.0).max(egui::vec2(0.0, ui.spacing().interact_size.y));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label)
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let fill = if selected {
        theme::accent_soft()
    } else if response.is_pointer_button_down_on() {
        theme::surface_active()
    } else if response.hovered() {
        theme::surface_hover()
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    let origin = rect.center() - galley.size() * 0.5;
    ui.painter().galley(origin, galley.clone(), theme::text());
    if row.emphasized {
        // 按实际字形范围裁剪后叠绘，确保只加粗正文且与原字形完全对齐。
        let start = index.chars().count();
        let end = start + row.text.chars().count();
        if let Some(line) = galley.rows.first()
            && let Some(first) = line.glyphs.get(start)
            && let Some(last) = line.glyphs.get(end - 1)
        {
            let clip = egui::Rect::from_min_max(
                origin + line.pos.to_vec2() + egui::vec2(first.pos.x, 0.0),
                origin
                    + line.pos.to_vec2()
                    + egui::vec2(last.max_x() + 0.4 * look.scale, line.size.y),
            );
            let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(clip));
            paint_bold_copy(&painter, origin, &galley, theme::text(), look);
        }
    }
    response
}

/// 沿用预览的合成粗体方式：原字形向右微移叠绘，不换字体、不改变字宽。
fn paint_bold_copy(
    painter: &egui::Painter,
    pos: egui::Pos2,
    galley: &std::sync::Arc<egui::Galley>,
    color: egui::Color32,
    look: Look,
) {
    painter.galley(
        pos + egui::vec2(0.4 * look.scale, 0.0),
        galley.clone(),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::super::{ImeSettings, keys::Action, keys::Navigation, table};
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    /// 候选窗依赖应用注册的字体族；轻量交互测试保留 egui 默认字面。
    fn configure_test_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.families.insert(
            egui::FontFamily::Name(theme::IME_FONT_FAMILY.into()),
            fonts.families[&egui::FontFamily::Proportional].clone(),
        );
        ctx.set_fonts(fonts);
    }

    fn candidate_ui() -> impl FnMut(&mut egui::Ui, &mut Ime) {
        let mut configured = false;
        move |ui, ime| {
            if !configured {
                configure_test_fonts(ui.ctx());
                configured = true;
                return;
            }
            ime.candidates_ui(ui.ctx());
        }
    }

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
        let mut harness = Harness::new_ui_state(candidate_ui(), ime("abcd"));
        harness.run();
        // 简码提示跟在词后；个人词条仅加粗候选正文。
        harness.get_by_label_contains("‹ab›");
        harness.get_by_label_contains("个人词");
        let mut harness = Harness::new_ui_state(candidate_ui(), ime("zz"));
        harness.run();
        harness.get_by_label("空码");
    }
    #[test]
    fn expanded_panel_lists_no_phrases_and_collapses_with_the_arrow() {
        let mut harness = Harness::new_ui_state(candidate_ui(), ime("ab"));
        harness.run();
        harness.get_by_label("展开候选").click();
        harness.run();
        // 「ab」开头只有词组：展开找字只列单字，词组还在收起后的候选条上。
        assert!(harness.state().browser.open);
        harness.get_by_label("没有可选的单字");
        assert!(harness.query_by_label_contains("个人词").is_none());
        harness.get_by_label("收起候选").click();
        harness.run();
        assert!(!harness.state().browser.open);
        harness.get_by_label_contains("公文");
    }

    #[test]
    fn single_code_has_no_expand_button() {
        let mut harness = Harness::new_ui_state(candidate_ui(), ime("a"));
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
                    configure_test_fonts(ui.ctx());
                    ui.memory_mut(|memory| memory.request_focus(editor));
                    *started = true;
                    return;
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
        let hanzi = |i: u8| char::from_u32(0x4E00 + u32::from(i)).unwrap().to_string();
        let mut text = String::from("ab,1=简码\n");
        for i in 0..40u8 {
            text.push_str(&format!(
                "ab{}{},1={}\n",
                char::from(b'a' + i / 26),
                char::from(b'a' + i % 26),
                hanzi(i)
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
        let mut harness = Harness::new_ui_state(candidate_ui(), state);
        harness.run();
        // 只列单字（词组「简码」不在），每行 5 格、露 4 行。第 5 行（第 21 个字起）
        // 滚动区备着画了，但被裁在可见区外，点不中。
        // 窗宽只够 5 个单字格：曾经表头按量到的网格宽摆箭头，两边互相撑到五百多像素。
        assert!(harness.query_by_label_contains("简码").is_none());
        let width = harness.state().window_size().unwrap().x;
        assert!(width < 400.0, "找字网格被撑宽到 {width}");
        harness.get_by_label(&hanzi(22)).click();
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
        harness.get_by_label(&hanzi(22)).click();
        harness.run();
        assert_eq!(harness.state().pending_commit, Some(hanzi(22)));
    }

    /// 用应用自己的字体与样式量：自定义中文字体行高大，仍需整行展示。
    /// 曾经只给滚动区上限、不钉高度，真机上只露出两行多。
    #[test]
    fn grid_shows_four_full_rows_with_app_fonts() {
        four_full_rows(|ctx| {
            theme::configure_style(ctx);
            theme::configure_fonts(ctx, &Default::default());
        });
    }

    /// 行带比候选格高时仍按实际行距滚动；用固定字面与加高的交互行复现，
    /// 避免字体或主题差异把第 4 行裁掉。
    #[test]
    fn grid_shows_four_full_rows_when_cells_are_shorter_than_the_row_band() {
        four_full_rows(|ctx| {
            theme::configure_style(ctx);
            configure_test_fonts(ctx);
            ctx.all_styles_mut(|style| style.spacing.interact_size.y = 60.0);
        });
    }

    /// 展开找字网格必须整行露出 `VISIBLE_ROWS` 行，第 5 行在窗底之外。
    fn four_full_rows(fonts: impl Fn(&egui::Context)) {
        let hanzi = |i: u32| char::from_u32(0x4E00 + i).unwrap().to_string();
        let mut text = String::new();
        for i in 0..40u8 {
            text.push_str(&format!(
                "ab{}{},1={}\n",
                char::from(b'a' + i / 26),
                char::from(b'a' + i % 26),
                hanzi(u32::from(i))
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
        let mut configured = false;
        let mut harness = Harness::new_ui_state(
            move |ui, ime: &mut Ime| {
                if !configured {
                    fonts(ui.ctx());
                    configured = true;
                    return;
                }
                ime.candidates_ui(ui.ctx());
            },
            state,
        );
        harness.run();
        harness.run();
        // 底栏前有上下各 5px 留白与 1px 分区线，不能把底栏算进滚动视口。
        let bottom = harness.get_by_label("40 个候选").rect().top() - 11.0;
        let rect = |i: u32| harness.get_by_label(&hanzi(i)).rect();
        assert!(
            rect(15).max.y <= bottom + 0.5,
            "第 4 行被裁：{:?}，窗底 {bottom}",
            rect(15)
        );
        assert!(
            rect(20).min.y >= bottom - 0.5,
            "第 5 行露出来了：{:?}",
            rect(20)
        );
    }

    /// 手动生成真实 egui 样张，检查两套配色、长辅码与字号缩放。
    #[test]
    #[ignore = "生成输入法扩展选词样张到 tmp/，供目视检查"]
    fn expanded_candidates_samples() {
        for (name, theme_name, percent) in [
            ("dark", crate::models::ThemeName::GruvboxDark, 100),
            ("light", crate::models::ThemeName::Claude, 100),
            ("large", crate::models::ThemeName::GruvboxDark, 200),
        ] {
            theme::set_current(theme_name);
            let ctx = egui::Context::default();
            theme::configure_fonts(&ctx, &Default::default());
            theme::configure_style(&ctx);
            theme::configure_icons(&ctx);
            ctx.set_pixels_per_point(2.0);
            let mut state = ime("uj");
            state.settings.font_percent = percent;
            let chars = "山鳝单删汕潸陕擅扇蟮杉赡闪姗膳衫缮挺舢跚善缮苫煽";
            let text: String = chars
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    let code = if i % 7 == 0 {
                        format!("uj{}z", char::from(b'a' + i as u8))
                    } else {
                        format!("uj{}", char::from(b'a' + i as u8))
                    };
                    format!("{code},1={c}\n")
                })
                .collect();
            state.table.base = table::parse(&text, false).entries;
            state.table.rebuild();
            state.refresh(false);
            state.expand_browser();
            state.anchor = Some(egui::Rect::from_min_size(
                egui::pos2(20.0, 20.0),
                egui::vec2(2.0, 16.0),
            ));
            let size = if percent == 200 {
                egui::vec2(760.0, 520.0)
            } else {
                egui::vec2(420.0, 330.0)
            };
            let mut canvas = crate::ui_snapshot::Canvas::default();
            for frame in 0..4 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        time: Some(frame as f64),
                        ..Default::default()
                    },
                    |_ui| state.candidates_ui(&ctx),
                );
                if frame < 3 {
                    canvas.absorb(&mut output.textures_delta);
                } else {
                    canvas.render(
                        &ctx,
                        output,
                        size,
                        theme::canvas(),
                        std::path::Path::new(&format!("tmp/ime-expanded-{name}.png")),
                    );
                }
            }
        }
        theme::set_current(crate::models::ThemeName::default());
    }
}
