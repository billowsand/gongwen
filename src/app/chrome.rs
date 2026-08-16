//! 窗口外壳：自绘标题栏、标签条、顶部菜单与状态栏。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::chrome`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{
    DOC_TAB_CHROME_WIDTH, DOC_TAB_MAX_WIDTH, DOC_TAB_MIN_WIDTH, DOC_TAB_TITLE_CHARS, GongwenApp,
    NavPage, TAB_HOVER_ANIM, TAB_PRESS_ANIM, TAB_SELECT_ANIM, TabRef, VersionScope,
    truncate_middle,
};
use crate::doc_import;
use crate::draft_page::{TOOLBAR_CONTROL_HEIGHT, toolbar_separator};
use crate::models::{ManuscriptStatus, ThemeName};
use crate::theme;
use crate::version;
use eframe::egui;

/// 自绘标题栏右侧窗口控制按钮的点击动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitlebarAction {
    Minimize,
    Maximize(bool),
    Close,
}

/// 自绘标题栏右侧的窗口控制按钮种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitlebarBtn {
    Minimize,
    Maximize,
    Close,
}

/// 标题栏按钮上绘制的图形符号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowGlyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// 用纯图形绘制标题栏按钮的符号（横线、方框、叠框、叉）。
///
/// 只画三四个几何形，不为三个小按钮引入额外图标资产；颜色跟随主题，
/// 悬停态由调用方决定底色后传入。
fn draw_window_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    glyph: WindowGlyph,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.4, color);
    match glyph {
        WindowGlyph::Minimize => {
            // 一条短横线。
            painter.rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(10.0, 1.6)),
                0.0,
                color,
            );
        }
        WindowGlyph::Maximize => {
            // 单个方框。
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(11.0, 11.0)),
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        WindowGlyph::Restore => {
            // 主框右下完整，副框左上只露上边和左边，形成 Windows 惯用的「还原」叠框。
            let main =
                egui::Rect::from_center_size(center + egui::vec2(1.5, 1.5), egui::vec2(10.0, 10.0));
            painter.rect_stroke(
                main,
                egui::CornerRadius::same(1),
                stroke,
                egui::StrokeKind::Inside,
            );
            let sub =
                egui::Rect::from_center_size(center - egui::vec2(1.5, 1.5), egui::vec2(10.0, 10.0));
            painter.line_segment([sub.left_top(), sub.right_top()], stroke);
            painter.line_segment([sub.left_top(), sub.left_bottom()], stroke);
        }
        WindowGlyph::Close => {
            // 两条交叉斜线。
            let d = 4.0;
            painter.line_segment(
                [center - egui::vec2(d, d), center + egui::vec2(d, d)],
                stroke,
            );
            painter.line_segment(
                [center - egui::vec2(d, -d), center + egui::vec2(d, -d)],
                stroke,
            );
        }
    }
}

/// 底部状态栏的紧凑图标按钮：尺寸、内边距都按 Windows 状态栏风格收小。
fn status_icon_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon: theme::Icon,
    label: &str,
    tint: Option<egui::Color32>,
    badge: Option<usize>,
) -> egui::Response {
    let image = match tint {
        Some(color) => icon.image().tint(color),
        None => icon.image(),
    };
    let response = ui.add(
        egui::Button::image(image)
            .image_tint_follows_text_color(tint.is_none())
            .selected(selected)
            .frame_when_inactive(selected)
            .min_size(egui::vec2(24.0, 20.0))
            .corner_radius(egui::CornerRadius::same(4)),
    );
    // 计数徽章：按钮右上角的小圆角气泡，提示条数不用悬停也一眼可见。
    if let Some(count) = badge.filter(|count| *count > 0) {
        let rect = response.rect;
        let text = if count > 99 {
            "99+".to_string()
        } else {
            count.to_string()
        };
        let width = if text.len() > 1 { 17.0 } else { 12.0 };
        let center = egui::pos2(rect.right() - 4.0, rect.top() + 3.0);
        ui.painter().rect_filled(
            egui::Rect::from_center_size(center, egui::vec2(width, 12.0)),
            egui::CornerRadius::same(6),
            theme::warn(),
        );
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            &text,
            egui::FontId::proportional(9.0),
            egui::Color32::WHITE,
        );
    }
    response.on_hover_text(label)
}

impl GongwenApp {
    /// 无边框窗口的顶栏。
    ///
    /// macOS 使用左侧 AppKit 原生红黄绿按钮、居中标题和右侧快速操作；
    /// Windows/Linux 继续使用右侧自绘窗口按钮。
    pub(crate) fn window_titlebar(&mut self, ui: &mut egui::Ui) {
        const HEIGHT: f32 = 34.0;
        const BTN_W: f32 = 46.0;

        let ctx = ui.ctx().clone();
        let maximized = ctx
            .input(|input| input.viewport().maximized)
            .unwrap_or(false);
        // 失焦时标题和图标弱化，提示窗口不在前台。
        let focused = ctx.input(|input| input.viewport().focused).unwrap_or(true);
        let title_color = if focused {
            theme::text_soft()
        } else {
            theme::text_muted()
        };

        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), HEIGHT),
            egui::Sense::hover(),
        );

        if let Some(metrics) = self.macos_titlebar_metrics {
            self.macos_titlebar(ui, rect, metrics.reserved_width, title_color);
            return;
        }

        // 左侧标题区：空白处可拖拽移动窗口，双击切换最大化。
        let title_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 14.0, rect.top()),
            egui::pos2(rect.right() - 3.0 * BTN_W, rect.bottom()),
        );
        let mut text_right = title_rect.left();
        if title_rect.width() > 60.0 {
            let brand_rect = egui::Rect::from_center_size(
                title_rect.left_center() + egui::vec2(9.0, 0.0),
                egui::vec2(18.0, 18.0),
            );
            let brand_color = if focused {
                theme::accent()
            } else {
                title_color
            };
            ui.put(
                brand_rect,
                theme::Icon::BrandMark.image_sized(18.0).tint(brand_color),
            )
            .on_hover_text(version::APP_TITLE);
            text_right = brand_rect.right();
        }
        // 快速访问工具栏：仿 Word 挂在标题栏上，与停在哪个分区卡无关。
        // 只在活动标签是稿件时出现——设置页、词库页上它们没有作用对象。
        let mut quick_rect: Option<egui::Rect> = None;
        if self.showing_doc() {
            let left = text_right + 18.0;
            let available = egui::Rect::from_min_max(
                egui::pos2(left, rect.top() + 4.0),
                egui::pos2(title_rect.right() - 8.0, rect.bottom() - 4.0),
            );
            if available.width() > 120.0 {
                let mut quick = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(available)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                quick.spacing_mut().item_spacing.x = 2.0;
                let labeled = rect.width() >= 1_100.0;
                self.titlebar_quick_access(&mut quick, labeled);
                quick_rect = Some(quick.min_rect());
            }
        }
        // 拖拽区绕开快速访问那几枚按钮：两侧各留一段，中间让给按钮。
        let gap = quick_rect.map_or(title_rect.right()..title_rect.right(), |quick| {
            (quick.left() - 4.0)..(quick.right() + 4.0)
        });
        for (index, zone) in [
            egui::Rect::from_min_max(title_rect.min, egui::pos2(gap.start, title_rect.bottom())),
            egui::Rect::from_min_max(egui::pos2(gap.end, title_rect.top()), title_rect.max),
        ]
        .into_iter()
        .enumerate()
        {
            if zone.width() < 4.0 {
                continue;
            }
            let drag = ui.interact(
                zone,
                ui.id().with(("titlebar_drag", index)),
                egui::Sense::click_and_drag(),
            );
            if drag.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            } else if drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }

        // Windows/Linux 右侧三个窗口控制按钮：最小化 / 最大化·还原 / 关闭。
        let mut action: Option<TitlebarAction> = None;
        for (index, kind) in [
            TitlebarBtn::Minimize,
            TitlebarBtn::Maximize,
            TitlebarBtn::Close,
        ]
        .into_iter()
        .enumerate()
        {
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - (3 - index) as f32 * BTN_W, rect.top()),
                egui::vec2(BTN_W, HEIGHT),
            );
            let response = ui
                .interact(
                    btn_rect,
                    ui.id().with(("titlebar_btn", index)),
                    egui::Sense::click(),
                )
                .on_hover_text(match kind {
                    TitlebarBtn::Minimize => "最小化",
                    TitlebarBtn::Maximize => {
                        if maximized {
                            "还原"
                        } else {
                            "最大化"
                        }
                    }
                    TitlebarBtn::Close => "关闭",
                });
            let is_close = kind == TitlebarBtn::Close;
            let hovered = response.hovered();
            let pressed = response.is_pointer_button_down_on();
            let fill = if hovered {
                if is_close {
                    // 关闭键按下再加深一档，与其余按钮的按压手感一致。
                    if pressed {
                        theme::danger().gamma_multiply(0.85)
                    } else {
                        theme::danger()
                    }
                } else if pressed {
                    theme::surface_active()
                } else {
                    theme::surface_hover()
                }
            } else {
                egui::Color32::TRANSPARENT
            };
            if fill != egui::Color32::TRANSPARENT {
                ui.painter().rect_filled(btn_rect, 0.0, fill);
            }
            // 关闭键悬停用红底白字（Windows 惯例），其余按钮保持文字色。
            let icon_color = if is_close && hovered {
                egui::Color32::WHITE
            } else {
                title_color
            };
            let glyph = match kind {
                TitlebarBtn::Minimize => WindowGlyph::Minimize,
                TitlebarBtn::Maximize if maximized => WindowGlyph::Restore,
                TitlebarBtn::Maximize => WindowGlyph::Maximize,
                TitlebarBtn::Close => WindowGlyph::Close,
            };
            draw_window_glyph(ui.painter(), btn_rect.center(), glyph, icon_color);
            if response.clicked() {
                action = Some(match kind {
                    TitlebarBtn::Minimize => TitlebarAction::Minimize,
                    TitlebarBtn::Maximize => TitlebarAction::Maximize(!maximized),
                    TitlebarBtn::Close => TitlebarAction::Close,
                });
            }
        }
        match action {
            Some(TitlebarAction::Minimize) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            Some(TitlebarAction::Maximize(next)) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(next));
            }
            Some(TitlebarAction::Close) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
    }

    /// macOS 顶栏：左侧红黄绿由 AppKit 原生视图绘制，egui 只负责
    /// 中间的当前标题、可拖拽区和右侧稿件快速操作。
    pub(crate) fn macos_titlebar(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        native_controls_width: f32,
        title_color: egui::Color32,
    ) {
        const RIGHT_MARGIN: f32 = 12.0;
        const QUICK_WIDTH_COMPACT: f32 = 102.0;
        const QUICK_WIDTH_LABELED: f32 = 284.0;

        let content_left = rect.left() + native_controls_width;
        let mut drag_right = rect.right() - RIGHT_MARGIN;

        // 快速操作是稿件上下文才有的工具，固定收在右边，避免和
        // 左侧红黄绿或中间标题抢位置。
        if self.showing_doc() {
            let labeled = rect.width() >= 1_100.0;
            let quick_width = if labeled {
                QUICK_WIDTH_LABELED
            } else {
                QUICK_WIDTH_COMPACT
            };
            let quick_left = (rect.right() - RIGHT_MARGIN - quick_width).max(content_left + 120.0);
            let available = egui::Rect::from_min_max(
                egui::pos2(quick_left, rect.top() + 4.0),
                egui::pos2(rect.right() - RIGHT_MARGIN, rect.bottom() - 4.0),
            );
            if available.width() >= 76.0 {
                let mut quick = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(available)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                quick.spacing_mut().item_spacing.x = 2.0;
                self.titlebar_quick_access(&mut quick, labeled);
                drag_right = available.left() - 8.0;
            }
        }

        // 按窗口整体的几何中心摆标题，不因左右控件宽度不对称而偏移。
        // 剪裁区对称收紧，窄窗口时不会压到原生按钮或快速操作。
        let center_x = rect.center().x;
        let half_width = (center_x - content_left)
            .min(drag_right - center_x)
            .max(0.0);
        if half_width > 40.0 {
            let (mark, active_title) = self.tab_label(self.active_tab);
            let active_title = if active_title.is_empty() {
                version::APP_TITLE.to_string()
            } else if mark.is_empty() {
                active_title
            } else {
                format!("{mark} {active_title}")
            };
            let clip = egui::Rect::from_min_max(
                egui::pos2(center_x - half_width, rect.top()),
                egui::pos2(center_x + half_width, rect.bottom()),
            );
            ui.painter().with_clip_rect(clip).text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                truncate_middle(&active_title, 42),
                egui::TextStyle::Body.resolve(ui.style()),
                title_color,
            );
        }

        let drag_rect = egui::Rect::from_min_max(
            egui::pos2(content_left, rect.top()),
            egui::pos2(drag_right, rect.bottom()),
        );
        if drag_rect.width() > 4.0 {
            let drag = ui.interact(
                drag_rect,
                ui.id().with("macos_titlebar_drag"),
                egui::Sense::click_and_drag(),
            );
            if drag.double_clicked() {
                crate::macos_window::perform_titlebar_double_click();
            } else if drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }
    }

    /// 标题栏上的快速访问：保存、提交版本、导出。这三件事跟当前停在哪个分区卡
    /// 无关，任何时候都该够得着，所以仿 Word 挂在标题栏上而不是放进功能区。
    /// 宽窗口显示短文字，窄窗口自动退回纯图标；保存键上的小圆点表示有未写回
    /// 稿件库的改动。这样高频动作第一眼可见，同时不牺牲窄窗口的标题空间。
    pub(crate) fn titlebar_quick_access(&mut self, ui: &mut egui::Ui, labeled: bool) {
        let Some(doc) = self.active_doc_ref() else {
            return;
        };
        let editable = !doc.read_only();
        let saved = doc.manuscript_id.is_some();
        let manuscript_id = doc.manuscript_id;
        let ready_to_export = !doc.busy && !doc.generated_markdown.trim().is_empty();
        let dirty = doc.is_dirty();
        let save_shortcut = theme::primary_shortcut("S");
        let save_hint = if saved {
            format!("保存：写回稿件库中这条记录（{save_shortcut}）")
        } else {
            format!("保存：在稿件库中新建一条草稿记录（{save_shortcut}）")
        };

        let save = if labeled {
            ui.add_enabled(editable, theme::icon_text_button(theme::Icon::Save, "保存"))
                .on_hover_text(&save_hint)
        } else {
            theme::icon_button_enabled(ui, editable, theme::Icon::Save, &save_hint)
        };
        if dirty {
            // 脏标记：强调色圆点外套一圈与标题栏同色的环，在浅色底上比
            // 光秃秃的 3px 圆点醒目得多。
            let center = save.rect.right_top() + egui::vec2(-5.0, 5.0);
            ui.painter().circle_filled(center, 4.5, theme::surface());
            ui.painter().circle_filled(center, 3.0, theme::accent());
        }
        if save.clicked() {
            self.save_to_manuscript_library();
        }
        let commit_hint = "提交版本：把当前内容固化为一个新版本";
        let commit = if labeled {
            ui.add_enabled(
                saved && editable,
                theme::icon_text_button(theme::Icon::GitCommit, "提交版本"),
            )
            .on_hover_text(commit_hint)
        } else {
            theme::icon_button_enabled(ui, saved && editable, theme::Icon::GitCommit, commit_hint)
        };
        if commit.clicked()
            && let Some(id) = manuscript_id
        {
            self.open_version_commit(VersionScope::Manuscript(id));
        }
        let export_hint = "导出：按设置里勾选的格式出文件";
        let export = if labeled {
            ui.add_enabled(
                ready_to_export,
                theme::icon_text_button(theme::Icon::FileDown, "导出"),
            )
            .on_hover_text(export_hint)
        } else {
            theme::icon_button_enabled(ui, ready_to_export, theme::Icon::FileDown, export_hint)
        };
        if export.clicked() {
            self.draft_page().start_export_current();
        }
    }

    /// 无边框窗口的自绘缩放边框。
    ///
    /// `with_decorations(false)` 之后，winit 会用自定义 `WM_NCCALCSIZE` 把非客户区
    /// 吃掉，DefWindowProc 对窗口四周几乎一律返回 `HTCLIENT`，系统那圈缩放边框只
    /// 剩一两个像素，鼠标基本压不住。这里在最上层铺八个命中区（四边 + 四角）自己
    /// 接管：悬停换缩放光标，按下就把后续交给系统的 `BeginResize`。
    ///
    /// 每个命中区各自是一个窄 `Area`，不能合成一个铺满窗口的大 `Area`：
    /// `Areas::layer_id_at` 只认最上面那层，一个满屏的可交互层会让底下所有
    /// `ScrollArea` 收不到滚轮、tooltip 也不再弹出。
    pub(crate) fn window_resize_borders(&mut self, ctx: &egui::Context) {
        use egui::ResizeDirection as Dir;
        /// 四边命中带宽度。
        const EDGE: f32 = 6.0;
        /// 四角命中区宽度，比边宽一档，斜向缩放好抓。
        const CORNER: f32 = 16.0;

        // 最大化时没有可拖的边，也免得白白盖住贴边的控件。
        if ctx
            .input(|input| input.viewport().maximized)
            .unwrap_or(false)
        {
            return;
        }
        // 取整个窗口区域而非 content_rect：缩放边框就是要压在最外那一圈上。
        let screen = ctx.viewport_rect();
        if screen.width() < 4.0 * CORNER || screen.height() < 4.0 * CORNER {
            return;
        }

        // (名字, 命中矩形, 方向, 光标)。竖边让开上下两角，四角单独铺。
        let x0 = screen.left();
        let x1 = screen.right();
        let y0 = screen.top();
        let y1 = screen.bottom();
        let zones: [(&str, egui::Rect, Dir, egui::CursorIcon); 8] = [
            (
                "n",
                egui::Rect::from_min_max(
                    egui::pos2(x0 + CORNER, y0),
                    egui::pos2(x1 - CORNER, y0 + EDGE),
                ),
                Dir::North,
                egui::CursorIcon::ResizeNorth,
            ),
            (
                "s",
                egui::Rect::from_min_max(
                    egui::pos2(x0 + CORNER, y1 - EDGE),
                    egui::pos2(x1 - CORNER, y1),
                ),
                Dir::South,
                egui::CursorIcon::ResizeSouth,
            ),
            (
                "w",
                egui::Rect::from_min_max(
                    egui::pos2(x0, y0 + CORNER),
                    egui::pos2(x0 + EDGE, y1 - CORNER),
                ),
                Dir::West,
                egui::CursorIcon::ResizeWest,
            ),
            (
                "e",
                egui::Rect::from_min_max(
                    egui::pos2(x1 - EDGE, y0 + CORNER),
                    egui::pos2(x1, y1 - CORNER),
                ),
                Dir::East,
                egui::CursorIcon::ResizeEast,
            ),
            (
                "nw",
                egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x0 + CORNER, y0 + CORNER)),
                Dir::NorthWest,
                egui::CursorIcon::ResizeNwSe,
            ),
            (
                "ne",
                egui::Rect::from_min_max(egui::pos2(x1 - CORNER, y0), egui::pos2(x1, y0 + CORNER)),
                Dir::NorthEast,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                "sw",
                egui::Rect::from_min_max(egui::pos2(x0, y1 - CORNER), egui::pos2(x0 + CORNER, y1)),
                Dir::SouthWest,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                "se",
                egui::Rect::from_min_max(egui::pos2(x1 - CORNER, y1 - CORNER), egui::pos2(x1, y1)),
                Dir::SouthEast,
                egui::CursorIcon::ResizeNwSe,
            ),
        ];

        for (name, rect, direction, cursor) in zones {
            egui::Area::new(egui::Id::new(("window_resize_zone", name)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .constrain(false)
                .interactable(true)
                .show(ctx, |ui| {
                    // 撑开 Area 自身的矩形，`layer_id_at` 才认得这一块。
                    ui.set_min_size(rect.size());
                    // 只感知 drag：纯拖拽控件按下当帧就算起拖（见 egui interaction.rs），
                    // 不必先挪够阈值，手感与系统边框一致。
                    let response = ui.interact(rect, ui.id().with("hit"), egui::Sense::drag());
                    if response.hovered() || response.dragged() {
                        ui.ctx().set_cursor_icon(cursor);
                    }
                    if response.drag_started() {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                    }
                });
        }
    }

    /// 顶格一整行：左边是菜单按钮，右边依次排开所有标签。稿件和导航页共用
    /// 这一条，界面纵向只让出一行。
    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.app_menu_button(ui);
            toolbar_separator(ui);
            self.tab_strip(ui);
        });
    }

    /// 左上角的菜单：应用图标 + 「菜单」文字，点开是五个常驻页面。
    /// 入口本身带文字而非纯图标——过去只有一枚汉堡图标，五个常驻页面和
    /// 新建公文这些高频入口藏在里面几乎不可见。
    pub(crate) fn app_menu_button(&mut self, ui: &mut egui::Ui) {
        let mut open: Option<NavPage> = None;
        egui::containers::menu::MenuButton::from_button(
            egui::Button::image_and_text(
                theme::Icon::Menu
                    .image()
                    .fit_to_exact_size(egui::vec2(18.0, 18.0)),
                "菜单",
            )
            .image_tint_follows_text_color(true),
        )
        .ui(ui, |ui| {
            ui.set_min_width(148.0);
            // 菜单项按 Windows 惯例用 9pt（≈12px）的紧凑字号，与状态栏同档。
            for style in [egui::TextStyle::Button, egui::TextStyle::Small] {
                ui.style_mut().text_styles.insert(
                    style,
                    egui::FontId::new(
                        theme::font_sizes::SMALL,
                        egui::FontFamily::Proportional,
                    ),
                );
            }
            for page in [
                NavPage::Manuscript,
                NavPage::Vocabulary,
                NavPage::AiPrompts,
                NavPage::Knowledge,
                NavPage::Settings,
            ] {
                // 菜单高亮表达“当前所在页”，不是“这个页面曾经开成了标签”。
                // 后台打开但未激活的页面不应和当前页同时显示为选中。
                let active = self.tabs.get(self.active_tab) == Some(&TabRef::Page(page));
                if ui
                    .add(
                        theme::menu_item(page.icon(), page.label())
                            .selected(active)
                            // 选中项常显强调色淡底，而不是只改文字颜色：
                            // `frame(false)` 会让按钮在未悬停时不画任何背景，
                            // 选中态退化成「文字变色」，条目本身没有反应。
                            .frame_when_inactive(active),
                    )
                    .clicked()
                {
                    open = Some(page);
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .add(
                    theme::menu_item(theme::Icon::FilePlus, "新建空白公文").right_text(
                            egui::RichText::new(theme::primary_shortcut("N"))
                                .color(theme::text_muted())
                                .small(),
                        ),
                )
                .clicked()
            {
                open = None;
                self.new_blank_manuscript();
                ui.close();
            }
            if ui
                .add(
                    theme::menu_item(theme::Icon::FileUp, "从文档新建公文"),
                )
                .on_hover_text(format!(
                    "把 Word / Excel / PPT / ODF / RTF / EPUB / CSV 转成 Markdown 新开一篇稿件（可导入 {}）",
                    doc_import::supported_summary()
                ))
                .clicked()
            {
                open = None;
                self.new_manuscript_from_document();
                ui.close();
            }
            ui.separator();
            if ui
                .add(
                    theme::menu_item(theme::Icon::Book, "关于公文助手"),
                )
                .clicked()
            {
                open = None;
                self.about_window_open = true;
                ui.close();
            }
            // 外观主题子菜单：主题切换以前只能在设置页里找，放到菜单底部
            // 随手可换。主题项用 selectable_label，选中项自动带淡底高亮。
            ui.separator();
            egui::containers::menu::SubMenuButton::from_button(
                theme::menu_item(theme::Icon::Palette, "外观主题")
                    .right_text(egui::containers::menu::SubMenuButton::RIGHT_ARROW),
            )
            .ui(ui, |ui| {
                // 十二套主题按明暗分两段，中间用分隔线断开，菜单才扫得动。
                for (index, dark) in [false, true].into_iter().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    for name in ThemeName::ALL
                        .into_iter()
                        .filter(|name| theme::by_name(*name).dark == dark)
                    {
                        let palette = theme::by_name(name);
                        let selected = name == self.config.theme;
                        if ui
                            .add(theme::menu_selectable_item(selected, palette.label))
                            .on_hover_text(if selected {
                                "当前主题"
                            } else {
                                "切换后立即生效并保存"
                            })
                            .clicked()
                            && !selected
                        {
                            self.apply_theme(ui.ctx(), name);
                            ui.close();
                        }
                    }
                }
            });
        })
        .0
        .on_hover_text("稿件管理、标准词库、AI 管理与设置");
        if let Some(page) = open {
            self.open_page(page);
        }
    }

    /// 应用图标菜单下的"关于"弹窗：版本号、构建信息、依赖致谢。
    pub(crate) fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_window_open {
            theme::reset_window_anim(ctx, egui::Id::new("about_win_anim"));
            return;
        }
        let mut close_clicked = false;
        let win = egui::Window::new("关于公文助手")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(360.0)
            .open(&mut self.about_window_open)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.heading("公文助手");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.weak("版本");
                    ui.label(env!("CARGO_PKG_VERSION"));
                });
                ui.horizontal(|ui| {
                    ui.weak("构建目标");
                    ui.label(format!(
                        "{} {}",
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    ));
                });
                ui.horizontal(|ui| {
                    ui.weak("Rust 工具链");
                    ui.label(format!(
                        "rustc {}（运行时由 env! 决定）",
                        env!("CARGO_PKG_RUST_VERSION", "未知")
                    ));
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    "基于 egui 的离线公文写作助手：规范排版、AI 起草与优化、知识库检索、稿件管理。",
                );
                ui.add_space(4.0);
                ui.weak("本软件为内部使用工具，所有数据仅保存在本机。");
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    if ui.button("关闭").clicked() {
                        close_clicked = true;
                    }
                });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("about_win_anim"), &w.response);
        }
        if close_clicked {
            self.about_window_open = false;
        }
    }

    /// 标签栏：宽度不够时收进右端的溢出下拉，当前这格始终可见。
    pub(crate) fn tab_strip(&mut self, ui: &mut egui::Ui) {
        // 右端固定占位：新建按钮，多格时还要留出溢出下拉。
        let reserved = if self.tabs.len() > 1 { 104.0 } else { 48.0 };
        let usable = (ui.available_width() - reserved).max(DOC_TAB_MIN_WIDTH);
        let desired: Vec<f32> = (0..self.tabs.len())
            .map(|tab| self.tab_width(ui, tab))
            .collect();

        // 从左往右塞，塞不下的收进溢出下拉；当前这格必须留在可见区。
        let mut shown: Vec<usize> = Vec::new();
        let mut used = 0.0;
        for (tab, width) in desired.iter().enumerate() {
            if used + width > usable && !shown.is_empty() {
                break;
            }
            used += width;
            shown.push(tab);
        }
        if !shown.contains(&self.active_tab) && self.active_tab < self.tabs.len() {
            while used + desired[self.active_tab] > usable && shown.len() > 1 {
                used -= desired[shown.pop().expect("shown 非空")];
            }
            shown.push(self.active_tab);
            shown.sort_unstable();
        }
        // 仍然超宽（比如只剩一格特别长的标题）就按比例压回去。
        let total: f32 = shown.iter().map(|&tab| desired[tab]).sum();
        let scale = if total > usable { usable / total } else { 1.0 };

        let mut select: Option<usize> = None;
        let mut close: Option<usize> = None;
        ui.spacing_mut().item_spacing.x = 4.0;
        for &tab in &shown {
            let width = (desired[tab] * scale).max(DOC_TAB_MIN_WIDTH);
            let (clicked, closed) = self.tab_button(ui, tab, width);
            if clicked {
                select = Some(tab);
            }
            if closed {
                close = Some(tab);
            }
        }
        let hidden: Vec<usize> = (0..self.tabs.len())
            .filter(|tab| !shown.contains(tab))
            .collect();
        if !hidden.is_empty() {
            egui::ComboBox::from_id_salt("tab_overflow")
                .selected_text(format!("»{}", hidden.len()))
                .width(46.0)
                .show_ui(ui, |ui| {
                    for tab in hidden {
                        let (mark, title) = self.tab_label(tab);
                        if ui
                            .selectable_label(false, format!("{mark}{title}"))
                            .clicked()
                        {
                            select = Some(tab);
                        }
                    }
                })
                .response
                .on_hover_text("切换到其余已打开的标签");
        }
        let new_shortcut = format!("新建空白公文（{}）", theme::primary_shortcut("N"));
        if theme::icon_button(ui, theme::Icon::FilePlus, &new_shortcut).clicked() {
            self.new_blank_manuscript();
        }

        if let Some(tab) = select {
            self.activate_tab(tab);
        }
        if let Some(tab) = close {
            self.request_close_tab(tab);
        }
    }

    /// 标签上显示的（状态标记, 标题）。
    pub(crate) fn tab_label(&self, tab: usize) -> (&'static str, String) {
        match self.tabs.get(tab) {
            Some(TabRef::Doc(key)) => match self.doc_index_of_key(*key) {
                Some(index) => (self.docs[index].dirty_mark(), self.docs[index].title()),
                None => ("", "已关闭".to_string()),
            },
            Some(TabRef::Page(page)) => ("", page.label().to_string()),
            Some(TabRef::Pdf(key)) => match self.pdf_index_of_key(*key) {
                Some(index) => ("", self.pdfs[index].title().to_string()),
                None => ("", "已关闭 PDF".to_string()),
            },
            None => ("", String::new()),
        }
    }

    /// 一格标签想要多宽：标题实际排版宽度加上标记与关闭按钮的固定占位。
    pub(crate) fn tab_width(&self, ui: &egui::Ui, tab: usize) -> f32 {
        let (_, title) = self.tab_label(tab);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let text = ui
            .painter()
            .layout_no_wrap(
                truncate_middle(&title, DOC_TAB_TITLE_CHARS),
                font,
                theme::text(),
            )
            .size()
            .x;
        (text + DOC_TAB_CHROME_WIDTH).clamp(DOC_TAB_MIN_WIDTH, DOC_TAB_MAX_WIDTH)
    }

    /// 画一格标签，返回（是否点了标签体, 是否点了关闭）。
    ///
    /// 动效统一在这里，构成一套闭合的交互：
    /// - **悬停**：背景/边框/文字 120ms 变深，关闭按钮从低对比升到实色；
    /// - **移出**：同样时长平滑回到常态；
    /// - **按下**：背景 150ms 再深一档；
    /// - **选中**：整体填充主题色的胶囊 160ms 淡入，文字同步过渡到白字。
    ///
    /// 关闭按钮常态低对比常显，悬停它本身时渐变到危险红（选中态为白色）。
    /// 交互状态来自先占位拿到的 response，颜色才能在画背景之前算好。
    pub(crate) fn tab_button(&mut self, ui: &mut egui::Ui, tab: usize, width: f32) -> (bool, bool) {
        let selected = tab == self.active_tab;
        let (mark, title) = self.tab_label(tab);
        let (icon, hover, busy) = match self.tabs[tab] {
            TabRef::Doc(key) => match self.doc_index_of_key(key) {
                Some(index) => {
                    let doc = &self.docs[index];
                    // 只读稿件用生命周期图标代替脏标记，一眼看出这篇动不了。
                    let icon = match doc.record_status {
                        _ if !doc.read_only() => None,
                        ManuscriptStatus::Archived => Some(theme::Icon::Archive),
                        _ => Some(theme::Icon::Publish),
                    };
                    (icon, doc.tab_hover(), doc.busy)
                }
                None => (None, title.clone(), false),
            },
            TabRef::Page(page) => (Some(page.icon()), page.label().to_string(), false),
            TabRef::Pdf(key) => match self.pdf_index_of_key(key) {
                Some(index) => (
                    Some(theme::Icon::FileTypePdf),
                    self.pdfs[index].tab_hover(),
                    self.pdfs[index].busy(),
                ),
                None => (Some(theme::Icon::FileTypePdf), title.clone(), false),
            },
        };

        // 先占位拿到交互状态，才能在同一帧内驱动颜色动画。
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(width, TOOLBAR_CONTROL_HEIGHT),
            egui::Sense::click(),
        );

        // Context 是 Arc，克隆断开与 ui 的借用，new_child 才能拿到可变借用。
        let ctx = ui.ctx().clone();
        // hover/按下用纯几何判断而非 response.hovered()：后者会被更上层的
        // 标题、关闭按钮抢走交互（指针落在它们上面时标签反而算「未悬停」）。
        let hovered = ui.rect_contains_pointer(rect);
        let pressed = hovered && ctx.input(|i| i.pointer.primary_down());
        let hover_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_hover").with(tab),
            hovered,
            TAB_HOVER_ANIM,
        );
        let press_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_press").with(tab),
            pressed,
            TAB_PRESS_ANIM,
        );
        let sel_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_select").with(tab),
            selected,
            TAB_SELECT_ANIM,
        );

        // 背景：选中整体填充主题色加深档（胶囊），保证白字对比度；未选中从沉色经悬停色到按下的更深色。
        // 导航页标签（词库、设置等）用更浅的 surface 底 + 实色描边，与文档标签的
        // 沉色底区分——一眼看出「这是导航页，不是打开的文档」。
        let is_page = matches!(self.tabs[tab], TabRef::Page(_));
        let mut bg = (if is_page {
            theme::surface()
        } else {
            theme::surface_sunk()
        })
        .lerp_to_gamma(theme::accent_active(), sel_t);
        bg = bg.lerp_to_gamma(theme::surface_hover(), hover_t * (1.0 - sel_t));
        bg = bg.lerp_to_gamma(theme::surface_active(), press_t * (1.0 - sel_t));
        // 边框：悬停加深；选中与底色同色，视觉上收成无边框的实心胶囊。
        let border = (if is_page {
            theme::border_strong()
        } else {
            theme::border()
        })
        .lerp_to_gamma(theme::border_strong(), hover_t * (1.0 - sel_t))
        .lerp_to_gamma(theme::accent_active(), sel_t);
        // 文字：未选中深色，悬停加深一档；选中渐变到白字，保证在主题色底上可读。
        let text_color = theme::text_soft()
            .lerp_to_gamma(theme::text(), hover_t * (1.0 - sel_t))
            .lerp_to_gamma(egui::Color32::WHITE, sel_t);
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(TOOLBAR_CONTROL_HEIGHT as u8 / 2),
            bg,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );

        let mut clicked = false;
        let mut closed = false;

        // 内容区：图标/脏标记在左，关闭按钮钉死在右端，标题吃掉中间。
        let inner = rect.shrink2(egui::vec2(8.0, 4.0));
        let mut content = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        content.spacing_mut().item_spacing.x = 4.0;
        if busy {
            content.spinner();
        } else if !mark.is_empty() {
            // 跟背景/文字同步插值，别用 sel_t > 0.5 那种硬阈值，否则动画中途会跳一下。
            content.colored_label(
                theme::accent().lerp_to_gamma(egui::Color32::WHITE, sel_t),
                mark,
            );
        } else if let Some(icon) = icon {
            // Lucide 图标用 currentColor 描边，这里跟随文字色渐变。
            content.add(
                icon.image()
                    .tint(text_color)
                    .fit_to_exact_size(egui::vec2(14.0, 14.0)),
            );
        }
        let close_width = 18.0;
        let label_width = (content.available_width() - close_width).max(24.0);
        let label_response = content.add_sized(
            [label_width, TOOLBAR_CONTROL_HEIGHT - 8.0],
            egui::Label::new(
                egui::RichText::new(truncate_middle(&title, DOC_TAB_TITLE_CHARS)).color(text_color),
            )
            .truncate()
            .sense(egui::Sense::click()),
        );
        // 点击标签空白处也选中；中键关闭是浏览器/编辑器的习惯，任意位置都认。
        if label_response.clicked() || response.clicked() {
            clicked = true;
        }
        if label_response.middle_clicked() || response.middle_clicked() {
            closed = true;
        }
        label_response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(hover);
        // 关闭按钮：常态低对比**常显**（不悬停也看得见），悬停标签时升到
        // 实色、悬停它本身时渐变到危险红——三个档位各自明确。按钮钉死在
        // 内容右端、位置可精确预估，hover 检测才能在画按钮之前拿到。
        let close_rect = egui::Rect::from_min_size(
            egui::pos2(inner.right() - close_width, inner.center().y - 8.0),
            egui::vec2(close_width, 16.0),
        );
        let close_hover_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_close_hover").with(tab),
            content.rect_contains_pointer(close_rect),
            TAB_HOVER_ANIM,
        );
        let close_base = theme::text_muted()
            .lerp_to_gamma(egui::Color32::WHITE, sel_t)
            .gamma_multiply(0.55 + 0.45 * hover_t);
        // 悬停关闭键的目标色：未选中是危险红；选中态在主题色胶囊上改用浅色叉
        // （红叉压在主题色上看不清）。两者同样按 sel_t 插值，避免中途突变。
        let close_hover_color = theme::danger().lerp_to_gamma(theme::canvas(), sel_t);
        let close_color = close_base.lerp_to_gamma(close_hover_color, close_hover_t);
        if content
            .add(
                egui::Button::new(egui::RichText::new("×").color(close_color))
                    .frame(false)
                    .min_size(egui::vec2(close_width, 16.0)),
            )
            .on_hover_text("关闭这个标签")
            .clicked()
        {
            closed = true;
        }

        (clicked, closed)
    }

    /// 底部状态栏。整条只有一行：左边状态文案，中间模型名，右边仿 Zed 的抽屉入口。
    ///
    /// 这里的布局有一处必须守住的约束：横向布局的纵向对齐是 `Align::Center`，
    /// egui 会让行内控件填满**当前可用高度**（见 `Layout::next_frame_ignore_wrap`）。
    /// 底部面板的可用高度又来自上一帧量到的内容高度，所以状态栏里一旦出现第二行，
    /// 第一行就会吃掉整条面板，第二行再往上叠一截，面板每帧长高一次，状态栏便会
    /// 一路往上爬。因此这里只允许一个 `horizontal`，并把行高钉死。
    pub(crate) fn status_bar(&mut self, ui: &mut egui::Ui) {
        /// 状态栏行高，参照 Windows 状态栏取紧凑值。
        const ROW_HEIGHT: f32 = 22.0;
        /// 模型标签高度，必须小于等于行高，否则又会把面板顶高。
        const CHIP_HEIGHT: f32 = 20.0;

        ui.scope(|ui| {
            // 字号按 Windows 状态栏的习惯收小一档（9pt ≈ 12px）。
            for style in [
                egui::TextStyle::Body,
                egui::TextStyle::Button,
                egui::TextStyle::Small,
            ] {
                ui.style_mut().text_styles.insert(
                    style,
                    egui::FontId::new(theme::font_sizes::SMALL, egui::FontFamily::Proportional),
                );
            }
            ui.style_mut().spacing.button_padding = egui::vec2(4.0, 2.0);
            ui.style_mut().spacing.item_spacing = egui::vec2(4.0, 2.0);
            ui.style_mut().spacing.interact_size.y = ROW_HEIGHT;

            let status = self.status.clone();
            let model = self.config.lm_studio.model.trim().to_owned();
            let show_doc_controls = self.showing_doc();
            let active_doc = self.active_doc;
            let (versions_open, result_open, warnings_count, saved) = if show_doc_controls {
                self.docs
                    .get(active_doc)
                    .map(|doc| {
                        (
                            doc.versions_open,
                            doc.result_drawer_open,
                            doc.warnings.len(),
                            doc.manuscript_id.is_some(),
                        )
                    })
                    .unwrap_or((false, false, 0, false))
            } else {
                (false, false, 0, false)
            };

            let status_limit = (ui.available_width() * 0.38).clamp(160.0, 420.0);
            ui.horizontal(|ui| {
                // 行高写死：`set_height` 同时钉住上下限，行内控件才不会去填满面板高度。
                ui.set_height(ROW_HEIGHT);

                // 左：忙碌指示灯与状态文案。文案过长直接截断，不许把中间的模型名挤走。
                if self.any_busy() {
                    ui.spinner();
                } else {
                    theme::dot(ui, theme::success());
                }
                ui.add_space(2.0);
                ui.add_sized(
                    [status_limit, ROW_HEIGHT],
                    egui::Label::new(egui::RichText::new(status).color(theme::text_soft()))
                        .truncate(),
                );
                let left_bound = ui.min_rect().right();

                // 右：导出、审校、版本三个抽屉入口。只留图标，说明放在悬停里。
                let right_bound = ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !show_doc_controls {
                            return ui.max_rect().right();
                        }
                        let review_tip = if warnings_count > 0 {
                            format!("审校提示 {warnings_count} 条")
                        } else {
                            "审校通过".to_string()
                        };
                        let review_tint = if warnings_count > 0 {
                            theme::warn()
                        } else {
                            theme::success()
                        };
                        if status_icon_button(
                            ui,
                            result_open,
                            theme::Icon::SquareCheck,
                            &review_tip,
                            Some(review_tint),
                            Some(warnings_count),
                        )
                        .clicked()
                        {
                            let doc = &mut self.docs[active_doc];
                            doc.result_drawer_open = !doc.result_drawer_open;
                        }
                        let version_tip = if saved {
                            "版本历史".to_string()
                        } else {
                            "版本历史（先保存）".to_string()
                        };
                        if status_icon_button(
                            ui,
                            versions_open,
                            theme::Icon::History,
                            &version_tip,
                            None,
                            None,
                        )
                        .clicked()
                        {
                            self.docs[active_doc].versions_open =
                                !self.docs[active_doc].versions_open;
                        }
                        ui.min_rect().left()
                    })
                    .inner;

                // 中：模型名。画在左右两组之间的空档里，仍属于这一行，不额外占纵向空间。
                let gap_left = left_bound + 10.0;
                let gap_right = right_bound - 10.0;
                if gap_right - gap_left > 80.0 {
                    let model_text = if model.is_empty() {
                        "模型：未选择".to_string()
                    } else {
                        format!("模型：{}", truncate_middle(&model, 36))
                    };
                    let (fg, bg) = if model.is_empty() {
                        (theme::warn(), theme::warn_soft())
                    } else {
                        (theme::success(), theme::surface_sunk())
                    };
                    // 对齐整条状态栏的中线；只有窗口太窄、居中会压到两侧时才让位。
                    let chip_width = (gap_right - gap_left).min(420.0);
                    let center_x = ui
                        .max_rect()
                        .center()
                        .x
                        .clamp(gap_left + chip_width * 0.5, gap_right - chip_width * 0.5);
                    let chip_rect = egui::Rect::from_center_size(
                        egui::pos2(center_x, ui.max_rect().center().y),
                        egui::vec2(chip_width, CHIP_HEIGHT),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(chip_rect)
                            .layout(egui::Layout::top_down(egui::Align::Center)),
                        |ui| theme::chip(ui, &model_text, fg, bg),
                    );
                }
            });
        });
    }
}
