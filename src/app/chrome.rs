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
use crate::draft_page::{PreviewMode, TOOLBAR_CONTROL_HEIGHT};
use crate::models::{DiagramTheme, ManuscriptStatus, ThemeName};
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
/// 状态栏统一标记词表输入法。
fn ime_scheme_marker(_ime: &crate::models::ImeConfig) -> Option<String> {
    Some("词表".into())
}

/// 状态栏的输入法指示：中 / 英 + 方案标记。整块可点，点了切中英。
fn ime_chip(ui: &mut egui::Ui, english: bool, scheme: Option<&str>, height: f32) -> egui::Response {
    let (fg, bg) = if english {
        (theme::text_muted(), theme::surface_sunk())
    } else {
        (theme::accent(), theme::accent_soft())
    };
    let mode = if english { "英" } else { "中" };
    let text = match scheme {
        Some(scheme) => format!("{mode} {scheme}"),
        None => mode.to_string(),
    };
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .color(fg)
                .size(theme::font_sizes::SMALL),
        )
        .fill(bg)
        .stroke(egui::Stroke::NONE)
        .corner_radius(theme::chrome_radius(3))
        .min_size(egui::vec2(0.0, height)),
    )
    .on_hover_text("输入法：点一下切换中 / 英（与单击 Shift 相同）")
}

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
            .corner_radius(theme::chrome_radius(4)),
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
            theme::chrome_radius(6),
            theme::warn(),
        );
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            &text,
            egui::FontId::proportional(9.0),
            theme::accent_text(),
        );
    }
    response.on_hover_text(label)
}

/// 状态栏里带弹层的条目：外观与 [`status_icon_button`] 一致，只是点开的是菜单。
fn status_menu_button<'a>(selected: bool, content: egui::Button<'a>) -> egui::Button<'a> {
    content
        .selected(selected)
        .frame_when_inactive(selected)
        .corner_radius(theme::chrome_radius(4))
}

/// 标签关闭键的圆形热区直径。18 px 在 28 px 高的胶囊里上下各余 5 px，
/// 悬停底圆不会顶到描边，热区又够一次点准。
const CLOSE_HIT: f32 = 18.0;
/// 叉本身的边长。热区四周各留 3 px，悬停时的底圆才像个托盘而不是描边。
const CLOSE_ICON: f32 = 12.0;
/// 标题与关闭键之间的留白，免得长标题的省略号贴到叉上。
const CLOSE_GAP: f32 = 4.0;
/// 关闭键右侧到胶囊边缘的留白，与内容区左侧内边距取同一个值，左右看齐。
const CLOSE_MARGIN: f32 = 8.0;

/// 标签关闭键钉死的位置：右端留 `CLOSE_MARGIN`，竖直方向对齐**整条胶囊**的中线。
///
/// 单独拎成函数是为了能脱开 egui 上下文测它——历史上这块出过两次错位：
/// 一次是按内容区（而非胶囊）的中线对齐，上下内边距不等时叉就偏低；
/// 一次是把按钮丢进横向流里，行带的 `interact_size` 把它顶高、`item_spacing`
/// 又把它右推，热区与画面各走各的。现在热区、底圆、叉共用这一个 rect。
fn close_button_rect(tab: egui::Rect) -> egui::Rect {
    egui::Rect::from_center_size(
        egui::pos2(tab.right() - CLOSE_MARGIN - CLOSE_HIT / 2.0, tab.center().y),
        egui::Vec2::splat(CLOSE_HIT),
    )
}

/// 顶栏纯图标按钮（新建、快捷查找、换肤）的边长：与标签胶囊同高，悬停底色是
/// 同直径的圆，观感对齐 Chrome 工具栏上的插件按钮。
const TOOL_BUTTON_SIZE: f32 = TOOLBAR_CONTROL_HEIGHT;
/// 相邻图标按钮之间的空隙。按钮之间靠悬停圆区分，不需要再留白。
const TOOL_BUTTON_GAP: f32 = 2.0;
/// 换肤弹层里每个主题的色样直径。
const THEME_SWATCH_SIZE: f32 = 14.0;

/// 合并后整条顶栏的高度：标签胶囊 28 + 上下各 6 —— 正是过去标签行的高度，
/// 标题栏那一行（34 px）整行省掉。
const RAIL_HEIGHT: f32 = 40.0;
/// 左侧固定簇宽度：10 内边距 + 菜单 40 + 8 + 分隔 1 + 7 + 保存 28 + 2 + 提交 28 + 10。
/// 标签从左簇之后起排；非稿件页保存/提交变灰而不消失，起点才不会跳。
const RAIL_LEFT_WIDTH: f32 = 134.0;
/// 右端工具组（检索 + 换肤）宽 58：28 + 2 + 28。
const RAIL_TOOLS_WIDTH: f32 = 58.0;
/// 工具组与窗口按钮之间：10 空白 + 1 分隔线 + 10 空白。
const RAIL_CONTROLS_GAP: f32 = 21.0;
/// 窗口按钮每格宽度，与 Windows 11 的标题栏按钮同档。
const RAIL_CONTROL_WIDTH: f32 = 46.0;
/// 标签条与右侧工具之间至少留出的拖拽带：窗口再窄也要留得下一段能拖的地方。
const RAIL_DRAG_MIN: f32 = 40.0;
/// 菜单按钮（品牌标 + 小箭头）的宽度与高度，仿 Figma 左上角「图标即菜单」。
const MENU_BUTTON_W: f32 = 40.0;
const MENU_BUTTON_H: f32 = 28.0;
/// 顶栏分隔线的高度。
const RAIL_DIVIDER_HEIGHT: f32 = 16.0;
/// 收进「词库」子菜单的四张词表，菜单与当前页高亮共用。
const LEXICON_PAGES: [NavPage; 4] = [
    NavPage::Vocabulary,
    NavPage::Proofread,
    NavPage::Lexicon,
    NavPage::ImeTable,
];

/// 顶栏纯图标按钮：常态只有图标，悬停、按下才铺一枚圆形底色。须放在
/// [`tool_button_scope`] 里加，否则全局按钮内边距会把它撑成扁块。
fn tool_button(icon: theme::Icon) -> egui::Button<'static> {
    egui::Button::image(if theme::smartisan::active() {
        icon.image().tint(theme::smartisan::chrome_ink())
    } else {
        icon.image()
    })
    .image_tint_follows_text_color(!theme::smartisan::active())
    .frame_when_inactive(false)
    .corner_radius(theme::chrome_radius((TOOL_BUTTON_SIZE / 2.0) as u8))
    .min_size(egui::Vec2::splat(TOOL_BUTTON_SIZE))
    .small()
}

/// 收紧图标按钮的内边距与间距。全局 `button_padding` 是 10×5、`item_spacing.x`
/// 是 8，16 px 图标会被撑成 36×30 的扁块，一排按钮之间再各隔 8 px，显得松散。
/// 弹出的菜单是独立的 Area，不继承这里的样式。
fn tool_button_scope<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let padding = (TOOL_BUTTON_SIZE - 16.0) / 2.0;
        ui.spacing_mut().button_padding = egui::vec2(padding, padding);
        ui.spacing_mut().item_spacing.x = TOOL_BUTTON_GAP;
        if theme::smartisan::active() {
            let widgets = &mut ui.visuals_mut().widgets;
            widgets.inactive.bg_stroke = egui::Stroke::NONE;
            widgets.hovered.bg_stroke = egui::Stroke::NONE;
            widgets.active.bg_stroke = egui::Stroke::NONE;
            widgets.hovered.weak_bg_fill = egui::Color32::from_white_alpha(18);
            widgets.hovered.bg_fill = egui::Color32::from_white_alpha(18);
            widgets.active.weak_bg_fill = egui::Color32::from_black_alpha(16);
            widgets.active.bg_fill = egui::Color32::from_black_alpha(16);
            widgets.inactive.corner_radius = egui::CornerRadius::same(5);
            widgets.hovered.corner_radius = egui::CornerRadius::same(5);
            widgets.active.corner_radius = egui::CornerRadius::same(5);
        }
        add(ui)
    })
    .inner
}

/// 菜单项按 Windows 惯例用 9pt（≈12px）的紧凑字号，与状态栏同档，条目之间只留
/// 2 px（全局 7 px 的行距放在菜单里太散）。子菜单、弹层各是独立的 Area，
/// 不继承父菜单的样式，每一层都要单独调一次。
fn compact_menu(ui: &mut egui::Ui) {
    for style in [egui::TextStyle::Button, egui::TextStyle::Small] {
        ui.style_mut().text_styles.insert(
            style,
            egui::FontId::new(theme::font_sizes::SMALL, egui::FontFamily::Proportional),
        );
    }
    ui.spacing_mut().item_spacing.y = 2.0;
}

/// 菜单条目右端的快捷键提示：弱化色，不和条目名抢视线。
fn muted_shortcut(text: &str) -> egui::RichText {
    egui::RichText::new(text).color(theme::text_muted()).small()
}

impl GongwenApp {
    /// 无边框窗口的顶栏：菜单、保存/提交、标签条、检索/换肤与窗口按钮共用一条 40 px 横带。
    ///
    /// 过去是两条面板：标题栏 34 px 放品牌标、快速访问、居中的稿名和窗口按钮，
    /// 标签行 44 px 放菜单、标签与两枚工具，中间还夹一道分隔线。合并后纵向让出
    /// 38 px，而标签仍是 28 px 胶囊、上下各留 6 px——正是过去标签行的内边距。
    ///
    /// 两条布局约束在这里立住：左簇宽度固定（保存/提交在非稿件页变灰而不消失），
    /// 标签起点不随页面切换跳；右端先留 40 px 拖拽带再放工具，窗口再窄也拖得动。
    pub(crate) fn window_titlebar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let native = self
            .macos_titlebar_metrics
            .map(|metrics| metrics.reserved_size(&ctx));
        let height = native.map_or(RAIL_HEIGHT, |size| RAIL_HEIGHT.max(size.y));
        let maximized = ctx
            .input(|input| input.viewport().maximized)
            .unwrap_or(false);
        let focused = ctx.input(|input| input.viewport().focused).unwrap_or(true);
        let hyprland = crate::linux_desktop::hyprland_session();
        let window_buttons = if native.is_some() {
            0
        } else if hyprland {
            1
        } else {
            3
        };

        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );

        // 稿纸主题：整条铺牛皮纸，底边留 2 px 纸色——原来标签行底部那条纸边，
        // 现在就是正文区的上沿。背景先占槽位，控件随后画在它上面。
        if theme::smartisan::active() {
            let background = ui.painter().add(egui::Shape::Noop);
            theme::smartisan::tab_background(ui.painter(), background, rect);
            ui.painter().hline(
                rect.x_range(),
                rect.top() + 0.5,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(28)),
            );
        }

        let zones = rail_zones(rect, native.map_or(0.0, |size| size.x), window_buttons);

        // 整条先铺一层拖拽区，后面加的控件自然盖在它上面：空白带拖动窗口、双击最大化。
        // 不必再像过去那样把拖拽区切成两段、绕开快速访问。
        let drag = ui.interact(
            rect,
            ui.scope_id().with("window_chrome_drag"),
            egui::Sense::click_and_drag(),
        );
        if drag.double_clicked() {
            if native.is_some() {
                crate::macos_window::perform_titlebar_double_click();
            } else if !hyprland {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
        } else if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        self.chrome_left_cluster(ui, rect, zones);
        self.chrome_tab_strip(ui, zones);
        self.chrome_tools(ui, zones);
        if window_buttons > 0 {
            let buttons: &[TitlebarBtn] = if hyprland {
                &[TitlebarBtn::Close]
            } else {
                &[
                    TitlebarBtn::Minimize,
                    TitlebarBtn::Maximize,
                    TitlebarBtn::Close,
                ]
            };
            chrome_window_buttons(ui, rect, zones, buttons, maximized, focused);
        }
    }

    /// 左簇：菜单（品牌标 + 小箭头）、分隔线、保存与提交。
    fn chrome_left_cluster(&mut self, ui: &mut egui::Ui, rect: egui::Rect, zones: RailZones) {
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    egui::pos2(zones.menu_left, rect.top()),
                    egui::vec2(RAIL_LEFT_WIDTH - 10.0, rect.height()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        left.spacing_mut().item_spacing.x = 0.0;
        let response = self.chrome_menu_button(&mut left);
        response.on_hover_text(format!(
            "{}：新建文档、稿件管理、词库与设置",
            version::APP_TITLE
        ));
        left.add_space(8.0);
        theme::divider_v(&mut left, RAIL_DIVIDER_HEIGHT);
        left.add_space(7.0);
        self.titlebar_quick_access(&mut left);
    }

    /// 左上角的菜单按钮：品牌标 + 一枚小箭头，仿 Figma 那种「图标即菜单」。
    ///
    /// 不走 `egui::Button`：它的高度下限是全局 `interact_size.y`（30 px），比标签
    /// 胶囊高 2 px。这里自己分配 40×28，和标签、右侧工具落在同一条基线上。
    fn chrome_menu_button(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(MENU_BUTTON_W, MENU_BUTTON_H),
            egui::Sense::click(),
        );
        let (mark, chevron) = if theme::smartisan::active() {
            (
                theme::smartisan::chrome_ink(),
                theme::smartisan::chrome_ink().gamma_multiply(0.72),
            )
        } else {
            (theme::accent(), theme::text_muted())
        };
        let pressed = response.is_pointer_button_down_on();
        if response.hovered() || pressed {
            let fill = if theme::smartisan::active() {
                if pressed {
                    egui::Color32::from_black_alpha(16)
                } else {
                    egui::Color32::from_white_alpha(18)
                }
            } else if pressed {
                theme::surface_active()
            } else {
                theme::surface_hover()
            };
            ui.painter()
                .rect_filled(rect, theme::chrome_radius(7), fill);
        }
        paint_menu_button(ui, rect, mark, chevron);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "菜单"));
        egui::Popup::menu(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.app_menu(ui));
        response
    }

    /// 标签条：从左簇之后排到右侧工具前的拖拽带。
    fn chrome_tab_strip(&mut self, ui: &mut egui::Ui, zones: RailZones) {
        if zones.tabs.width() < 1.0 {
            return;
        }
        let mut strip = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(zones.tabs)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        self.tab_strip(&mut strip);
    }

    /// 右端工具组：检索与换肤。窗口按钮前留一道短分隔线，关闭键不挨着功能键。
    fn chrome_tools(&mut self, ui: &mut egui::Ui, zones: RailZones) {
        let mut tools = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(zones.tools)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        tool_button_scope(&mut tools, |ui| {
            egui::containers::menu::MenuButton::from_button(tool_button(theme::Icon::Shirt))
                .ui(ui, |ui| self.theme_menu(ui))
                .0
                .on_hover_text("外观主题");
            if ui
                .add(tool_button(theme::Icon::Search))
                .on_hover_text(format!(
                    "快捷查找：稿件、单位人员与页面（{}）",
                    theme::primary_shortcut("K")
                ))
                .clicked()
            {
                self.open_quick_find();
            }
        });
        if let Some(divider) = zones.divider {
            ui.painter().rect_filled(divider, 0.0, theme::border());
        }
    }

    /// 顶栏的快速访问：保存、提交版本。
    ///
    /// 这两件事跟当前停在哪个分区卡无关，任何时候都该够得着，所以仿 Word 快速访问
    /// 工具栏挂在最左边、紧挨着菜单。只放图标、常态无底色；非稿件页也占住位置
    /// （变灰不可点），这样标签条的起点不会随页面切换左右跳。
    /// 保存键上的小圆点表示有未写回稿件库的改动。
    pub(crate) fn titlebar_quick_access(&mut self, ui: &mut egui::Ui) {
        // `active_doc` 记的是最后编辑的那篇稿，停在设置/词库页时它仍然有值；
        // 这里要问的是「当前标签是不是稿件」，所以先看 `showing_doc`。
        let state = if self.showing_doc() {
            self.active_doc_ref().map(|doc| {
                (
                    doc.read_only(),
                    doc.manuscript_id.is_some(),
                    doc.is_dirty(),
                    doc.manuscript_id,
                )
            })
        } else {
            None
        };
        let Some((read_only, saved, dirty, manuscript_id)) = state else {
            // 非稿件页：位置照样占住，只是变灰。
            theme::rail_icon_button(ui, false, theme::Icon::Save, "保存：当前不是稿件页");
            ui.add_space(2.0);
            theme::rail_icon_button(
                ui,
                false,
                theme::Icon::GitCommit,
                "提交版本：当前不是稿件页",
            );
            return;
        };
        let editable = !read_only;
        let save_shortcut = theme::primary_shortcut("S");
        let save_hint = if saved {
            format!("保存：写回稿件库中这条记录（{save_shortcut}）")
        } else {
            format!("保存：在稿件库中新建一条草稿记录（{save_shortcut}）")
        };
        let save = theme::rail_icon_button(ui, editable, theme::Icon::Save, &save_hint);
        if dirty {
            // 脏标记：强调色圆点外套一圈与顶栏同色的环，压在图标右上角也分得清。
            let center = save.rect.right_top() + egui::vec2(-5.0, 5.0);
            ui.painter().circle_filled(center, 4.0, theme::surface());
            ui.painter().circle_filled(center, 2.5, theme::accent());
        }
        if save.clicked() {
            self.save_to_manuscript_library_with_ctx(ui.ctx());
        }
        ui.add_space(2.0);
        let commit_hint = if saved {
            "提交版本：把当前内容固化为一个新版本"
        } else {
            "提交版本：先保存，再提交版本"
        };
        let commit =
            theme::rail_icon_button(ui, saved && editable, theme::Icon::GitCommit, commit_hint);
        if commit.clicked()
            && let Some(id) = manuscript_id
        {
            self.open_version_commit(VersionScope::Manuscript(id));
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

        // Hyprland 负责平铺窗口的尺寸；自绘边缘只会抢占贴边控件的命中区域。
        if crate::linux_desktop::hyprland_session() {
            return;
        }

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
            egui::Area::new(egui::Id::unique(("window_resize_zone", name)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .constrain(false)
                .interactable(true)
                .show(ctx, |ui| {
                    // 撑开 Area 自身的矩形，`layer_id_at` 才认得这一块。
                    ui.set_min_size(rect.size());
                    // 只感知 drag：纯拖拽控件按下当帧就算起拖（见 egui interaction.rs），
                    // 不必先挪够阈值，手感与系统边框一致。
                    let response =
                        ui.interact(rect, ui.scope_id().with("hit"), egui::Sense::drag());
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

    /// 左上角菜单的内容：新建入口、常驻页面、图表样式与设置。
    ///
    /// 入口是顶栏左端的品牌标（见 [`menu_button`]），不再单占「菜单」两个字；
    /// 悬停图标即可看到这组入口的说明。
    fn app_menu(&mut self, ui: &mut egui::Ui) {
        compact_menu(ui);
        ui.set_min_width(148.0);
        self.new_document_items(ui);
        ui.separator();
        self.nav_menu_item(ui, NavPage::Manuscript);
        self.nav_menu_item(ui, NavPage::IssueLedger);
        #[cfg(feature = "task-assistant")]
        self.nav_menu_item(ui, NavPage::TaskAssistant);
        // 四张词表同属「词库」，收进一个子菜单；当前页是其中之一时，
        // 子菜单入口也带选中底，顺着高亮就能找到所在位置。
        let lexicon_active = LEXICON_PAGES
            .iter()
            .any(|page| self.tabs.get(self.active_tab) == Some(&TabRef::Page(*page)));
        egui::containers::menu::SubMenuButton::from_button(
            theme::menu_item(theme::Icon::Book, "词库")
                .selected(lexicon_active)
                .frame_when_inactive(lexicon_active)
                .right_text(egui::containers::menu::SubMenuButton::arrow_atom(None)),
        )
        .ui(ui, |ui| {
            compact_menu(ui);
            ui.set_min_width(132.0);
            for page in LEXICON_PAGES {
                self.nav_menu_item(ui, page);
            }
        });
        #[cfg(feature = "ai-workbench")]
        self.nav_menu_item(ui, NavPage::AiPrompts);
        #[cfg(feature = "ai-workbench")]
        self.nav_menu_item(ui, NavPage::Knowledge);
        #[cfg(feature = "ai-workbench")]
        self.nav_menu_item(ui, NavPage::Wiki);
        ui.separator();
        // 外观主题在顶栏右端的换肤按钮里。图表样式管的是纸面上的图，
        // 导出也跟着变，留在菜单里挨着设置。
        egui::containers::menu::SubMenuButton::from_button(
            theme::menu_item(theme::Icon::GitCommit, "图表样式")
                .right_text(egui::containers::menu::SubMenuButton::arrow_atom(None)),
        )
        .ui(ui, |ui| {
            compact_menu(ui);
            for diagram_theme in DiagramTheme::ALL {
                let selected = diagram_theme == self.config.diagram_theme;
                if ui
                    .add(theme::menu_selectable_item(selected, diagram_theme.label()))
                    .on_hover_text(diagram_theme.hint())
                    .clicked()
                    && !selected
                {
                    self.apply_diagram_theme(diagram_theme);
                    ui.close();
                }
                // 「按文种自动」与四套具体配色分开两段。
                if diagram_theme == DiagramTheme::Auto {
                    ui.separator();
                }
            }
        });
        self.nav_menu_item(ui, NavPage::Settings);
        if self.vault_locked
            && ui
                .add(theme::menu_item(theme::Icon::Lock, "锁定窗口"))
                .on_hover_text("立即退回锁屏；闲置超时也会自动锁定。")
                .clicked()
        {
            self.request_lock();
            ui.close();
        }
        ui.separator();
        if ui
            .add(theme::menu_item(theme::Icon::BrandMark, "关于公文助手"))
            .clicked()
        {
            self.about_window_open = true;
            ui.close();
        }
    }

    /// 菜单里的一个常驻页面入口。
    fn nav_menu_item(&mut self, ui: &mut egui::Ui, page: NavPage) {
        // 菜单高亮表达“当前所在页”，不是“这个页面曾经开成了标签”。
        // 后台打开但未激活的页面不应和当前页同时显示为选中。
        let active = self.tabs.get(self.active_tab) == Some(&TabRef::Page(page));
        let item = theme::menu_item(page.icon(), page.label())
            .selected(active)
            // 选中项常显强调色淡底，而不是只改文字颜色：
            // `frame(false)` 会让按钮在未悬停时不画任何背景，
            // 选中态退化成「文字变色」，条目本身没有反应。
            .frame_when_inactive(active);
        if ui.add(item).clicked() {
            self.open_page(page);
            ui.close();
        }
    }

    /// 三个新建入口。主菜单与标签栏加号的右键菜单共用。
    fn new_document_items(&mut self, ui: &mut egui::Ui) {
        if ui
            .add(
                theme::menu_item(theme::Icon::FilePlus, "新建空白文档")
                    .right_text(muted_shortcut(&theme::primary_shortcut("N"))),
            )
            .clicked()
        {
            self.new_blank_manuscript();
            ui.close();
        }
        if ui
            .add(theme::menu_item(theme::Icon::FileUp, "从文件新建文档…"))
            .on_hover_text(format!(
                "把 Word / Excel / PPT / ODF / RTF / EPUB / CSV 转成 Markdown 新开一篇稿件（可导入 {}）",
                doc_import::supported_summary()
            ))
            .clicked()
        {
            self.new_manuscript_from_document();
            ui.close();
        }
        if ui
            .add(theme::menu_item(
                theme::Icon::Folder,
                "从文件夹新建研究报告…",
            ))
            .on_hover_text("按文件名升序合并文件夹第一层的 .md 文件，并导入本地图片与 BibTeX")
            .clicked()
        {
            self.new_research_manuscript_from_folder();
            ui.close();
        }
    }

    /// 换肤弹层：顶部一项「明暗跟随系统」，下面六套主题按明暗分两段，中间一道
    /// 分隔线。每行左侧一枚色样（主题底色的圆 + 强调色圆点），选中项常显淡底并
    /// 在右端打勾，点了立即生效并保存。色样画在按钮自己的 atom 里，悬停底色才
    /// 铺得满整行。
    fn theme_menu(&mut self, ui: &mut egui::Ui) {
        compact_menu(ui);
        ui.set_min_width(148.0);
        let follow = self.config.theme_follows_system;
        if ui
            .add(theme::menu_selectable_item(follow, "明暗跟随系统"))
            .on_hover_text("随桌面深浅模式在月白与北欧之间切换；点下面的主题改为手动选择")
            .clicked()
        {
            self.apply_theme_follow_system(ui.ctx(), !follow);
            ui.close();
        }
        ui.separator();
        let mut index = 0usize;
        let preset_group = |dark: bool| -> Vec<ThemeName> {
            ThemeName::ALL
                .into_iter()
                .filter(|name| theme::by_name(*name).dark == dark)
                .collect()
        };
        let omarchy_group: Vec<ThemeName> = theme::omarchy::available()
            .then_some(ThemeName::Omarchy)
            .into_iter()
            .collect();
        for (group, names) in [preset_group(false), preset_group(true), omarchy_group]
            .into_iter()
            .enumerate()
        {
            if names.is_empty() {
                continue;
            }
            if group > 0 {
                ui.separator();
            }
            for name in names {
                let palette = theme::by_name(name);
                let selected = name == self.applied_theme;
                let swatch = egui::IdSalt::new(("theme_swatch", index));
                index += 1;
                let mut item = egui::Button::new((
                    egui::Atom::custom(swatch, egui::Vec2::splat(THEME_SWATCH_SIZE)),
                    palette.label,
                ))
                .selected(selected)
                .frame_when_inactive(selected)
                .corner_radius(theme::chrome_radius(5))
                .min_size(egui::vec2(0.0, 26.0));
                if selected {
                    item = item
                        .right_text(theme::Icon::Check.image_sized(14.0))
                        .image_tint_follows_text_color(true);
                }
                let mut response = item.atom_ui(ui);
                if name == ThemeName::Omarchy
                    && let Some(omarchy) = theme::omarchy::theme_name()
                {
                    response.response = response.response.on_hover_text(format!(
                        "Omarchy 当前主题：{omarchy}。Omarchy 换主题后再点一次即可跟上。"
                    ));
                }
                if let Some(rect) = response.rect(swatch) {
                    let painter = ui.painter();
                    let radius = THEME_SWATCH_SIZE / 2.0;
                    painter.circle(
                        rect.center(),
                        radius - 0.5,
                        palette.canvas,
                        egui::Stroke::new(1.0, palette.border_strong),
                    );
                    painter.circle_filled(rect.center(), radius * 0.45, palette.accent);
                }
                if response.clicked() {
                    // 「跟随 Omarchy」已选中时再点也要重读 Omarchy 当前主题。
                    if !selected || name == ThemeName::Omarchy {
                        self.apply_theme(ui.ctx(), name);
                    }
                    ui.close();
                }
            }
        }
    }

    /// 应用图标菜单下的"关于"弹窗：版本号、构建信息、依赖致谢。
    pub(crate) fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_window_open {
            theme::reset_window_anim(ctx, egui::Id::unique("about_win_anim"));
            return;
        }
        let mut close_clicked = false;
        #[cfg(feature = "ai-workbench")]
        let mut start_check = false;
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
                        option_env!("CARGO_PKG_RUST_VERSION").unwrap_or("未知")
                    ));
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(if cfg!(feature = "ai-workbench") {
                    "公文助手 AI 全版 · 规范排版、智能写作、知识库与稿件管理。"
                } else {
                    "公文助手非 AI 版 · 规范排版、规则校对与稿件管理。"
                });
                ui.add_space(4.0);
                ui.weak("本软件为内部使用工具，所有数据仅保存在本机。");
                #[cfg(feature = "ai-workbench")]
                {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        match &self.update_check {
                            crate::update_check::UpdateCheckState::Checking => {
                                let _ = ui.add_enabled(false, egui::Button::new("正在检查…"));
                            }
                            _ => {
                                if ui.button("检查更新").clicked() {
                                    start_check = true;
                                }
                            }
                        }
                        ui.weak("只检查一次，不轮询");
                    });
                    match &self.update_check {
                        crate::update_check::UpdateCheckState::Idle
                        | crate::update_check::UpdateCheckState::Checking => {}
                        crate::update_check::UpdateCheckState::Latest => {
                            ui.add_space(4.0);
                            ui.label(format!("当前已是最新版本 v{}", env!("CARGO_PKG_VERSION")));
                        }
                        crate::update_check::UpdateCheckState::Available(info) => {
                            ui.add_space(4.0);
                            ui.label(format!(
                                "发现新版本：v{} → v{}",
                                env!("CARGO_PKG_VERSION"),
                                info.version
                            ));
                            ui.hyperlink_to("打开发布页", &info.url);
                            ui.weak("内网环境若无法访问发布页，请联系维护者获取安装包。");
                        }
                        crate::update_check::UpdateCheckState::Failed(error) => {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "无法连接更新服务：{}。可忽略，或到设置页修改更新检查地址。",
                                    error.trim_end_matches('。')
                                ))
                                .color(theme::danger()),
                            );
                        }
                    }
                }
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    if ui.button("关闭").clicked() {
                        close_clicked = true;
                    }
                });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::unique("about_win_anim"), &w.response);
        }
        #[cfg(feature = "ai-workbench")]
        if start_check {
            self.start_update_check(ctx);
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
                if let Some(last) = shown.pop() {
                    used -= desired[last];
                }
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
        for (position, &tab) in shown.iter().enumerate() {
            let width = (desired[tab] * scale).max(DOC_TAB_MIN_WIDTH);
            let (clicked, closed, rect) = self.tab_button(ui, tab, width);
            // 短分隔线只出现在相邻的未选中标签之间，纸页两肩保持干净。
            if theme::smartisan::active()
                && tab != self.active_tab
                && shown
                    .get(position + 1)
                    .is_some_and(|next| *next != self.active_tab)
            {
                ui.painter().vline(
                    rect.right() + 2.0,
                    (rect.top() + 8.0)..=(rect.bottom() - 8.0),
                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(42)),
                );
            }
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
        // 加号和浏览器一样单击就新开一篇空白稿；从文件、文件夹新建放在右键菜单
        // 和左上角主菜单里，不让最常用的操作多点一层。
        let new_tab = tool_button_scope(ui, |ui| {
            let mut button = tool_button(theme::Icon::Plus);
            if theme::smartisan::active() {
                button = button.frame_when_inactive(false).corner_radius(14);
                ui.visuals_mut().override_text_color = Some(theme::smartisan::chrome_ink());
                ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::NONE;
                ui.visuals_mut().widgets.active.bg_stroke = egui::Stroke::NONE;
            }
            ui.add(button)
        })
        .on_hover_text(format!(
            "新建空白文档（{}）\n右键可从文件或文件夹新建",
            theme::primary_shortcut("N")
        ));
        if new_tab.clicked() {
            self.new_blank_manuscript();
        }
        new_tab.context_menu(|ui| {
            compact_menu(ui);
            ui.set_min_width(148.0);
            self.new_document_items(ui);
        });

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
            Some(TabRef::DraftPdf(key)) => {
                ("", format!("PDF 预览 · {}", self.doc_title_of_key(*key)))
            }
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
                truncate_middle(&title, self.tab_title_chars(tab)),
                font,
                theme::text(),
            )
            .size()
            .x;
        let max_width = if self.paper_document_tab(tab) {
            370.0
        } else {
            DOC_TAB_MAX_WIDTH
        };
        (text + DOC_TAB_CHROME_WIDTH).clamp(DOC_TAB_MIN_WIDTH, max_width)
    }

    fn paper_document_tab(&self, tab: usize) -> bool {
        theme::smartisan::active() && matches!(self.tabs.get(tab), Some(TabRef::Doc(_)))
    }

    fn tab_title_chars(&self, tab: usize) -> usize {
        if self.paper_document_tab(tab) {
            22
        } else {
            DOC_TAB_TITLE_CHARS
        }
    }

    /// 画一格标签，返回（是否点了标签体, 是否点了关闭, 实际标签区域）。
    ///
    /// 动效统一在这里，构成一套闭合的交互：
    /// - **悬停**：背景/边框/文字 120ms 变深，关闭按钮从低对比升到实色；
    /// - **移出**：同样时长平滑回到常态；
    /// - **按下**：背景 150ms 再深一档；
    /// - **选中**：整体填充主题色的胶囊 160ms 淡入，文字同步过渡到白字。
    ///
    /// 关闭按钮常态低对比常显，悬停它本身时渐变到危险红（选中态为白色）。
    /// 交互状态来自先占位拿到的 response，颜色才能在画背景之前算好。
    pub(crate) fn tab_button(
        &mut self,
        ui: &mut egui::Ui,
        tab: usize,
        width: f32,
    ) -> (bool, bool, egui::Rect) {
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
            TabRef::DraftPdf(key) => (
                Some(theme::Icon::FileTypePdf),
                title.clone(),
                self.draft_pdfs.is_compiling(key),
            ),
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
            egui::Id::unique("tab_hover").with(tab),
            hovered,
            TAB_HOVER_ANIM,
        );
        let press_t = ctx.animate_bool_with_time(
            egui::Id::unique("tab_press").with(tab),
            pressed,
            TAB_PRESS_ANIM,
        );
        let sel_t = ctx.animate_bool_with_time(
            egui::Id::unique("tab_select").with(tab),
            selected,
            TAB_SELECT_ANIM,
        );

        // 直角外壳选中态使用浅强调底与底部标记；其它主题保持实心胶囊。
        // 未选中从沉色经悬停色到按下的更深色。
        // 导航页标签（词库、设置等）用更浅的 surface 底 + 实色描边，与文档标签的
        // 沉色底区分——一眼看出「这是导航页，不是打开的文档」。
        let rectangular = theme::square_chrome() || theme::smartisan::active();
        let selected_fill = if theme::smartisan::active() {
            theme::surface()
        } else if rectangular {
            theme::accent_soft()
        } else {
            theme::accent_active()
        };
        let selected_border = if rectangular {
            theme::border()
        } else {
            theme::accent_active()
        };
        let selected_text = if rectangular {
            theme::accent()
        } else {
            theme::accent_text()
        };
        let is_page = matches!(self.tabs[tab], TabRef::Page(_));
        let mut bg = (if is_page {
            theme::surface()
        } else {
            theme::surface_sunk()
        })
        .lerp_to_gamma(selected_fill, sel_t);
        bg = bg.lerp_to_gamma(theme::surface_hover(), hover_t * (1.0 - sel_t));
        bg = bg.lerp_to_gamma(theme::surface_active(), press_t * (1.0 - sel_t));
        // 边框：悬停加深；直角外壳常显细线，其它主题选中时与底色同色。
        let border = (if is_page {
            theme::border_strong()
        } else {
            theme::border()
        })
        .lerp_to_gamma(theme::border_strong(), hover_t * (1.0 - sel_t))
        .lerp_to_gamma(selected_border, sel_t);
        // 文字：未选中深色，悬停加深一档；选中按各主题的背景取对比色。
        let text_color = if theme::smartisan::active() {
            if selected {
                theme::text()
            } else {
                theme::smartisan::chrome_ink()
            }
        } else {
            theme::text_soft()
                .lerp_to_gamma(theme::text(), hover_t * (1.0 - sel_t))
                .lerp_to_gamma(selected_text, sel_t)
        };
        if theme::smartisan::active() {
            theme::smartisan::document_tab_to(
                ui.painter(),
                rect,
                selected,
                hovered,
                pressed,
                ui.clip_rect().bottom() - 2.0,
            );
        } else {
            ui.painter().rect(
                rect,
                theme::chrome_radius(TOOLBAR_CONTROL_HEIGHT as u8 / 2),
                bg,
                egui::Stroke::new(1.0, border),
                egui::StrokeKind::Inside,
            );
        }

        if theme::square_chrome() && sel_t > 0.0 {
            ui.painter().line_segment(
                [
                    rect.left_bottom() - egui::vec2(0.0, 1.0),
                    rect.right_bottom() - egui::vec2(0.0, 1.0),
                ],
                egui::Stroke::new(2.0, theme::accent().gamma_multiply(sel_t)),
            );
        }

        let mut clicked = false;
        let mut closed = false;

        // 内容区：图标/脏标记在左，关闭按钮钉死在右端，标题吃掉中间。
        let mut inner = rect.shrink2(egui::vec2(8.0, 4.0));
        if selected && self.paper_document_tab(tab) {
            // 纸签左侧的短色签比整格强调底更安静，文字与色签保持间距。
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    rect.min + egui::vec2(3.0, 6.0),
                    egui::pos2(rect.left() + 6.0, rect.bottom() - 5.0),
                ),
                1,
                theme::smartisan::INDEX_MARK,
            );
            inner.min.x += 3.0;
        }
        let mut content = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        content.spacing_mut().item_spacing.x = 4.0;
        if busy {
            // 尺寸咬死 14px，与下面的图标一样大：忙碌时转圈只是顶替图标的位置，
            // 不会把这行撑高（egui 自带 spinner 按 interact_size 取 30px，会把
            // 圆圈顶出胶囊、同时把标题挤得往下掉）。颜色跟着文字走，选中态的
            // 主题色胶囊上自动变白。
            theme::spinner(&mut content, 14.0, text_color);
        } else if !mark.is_empty() {
            // 跟背景/文字同步插值，别用 sel_t > 0.5 那种硬阈值，否则动画中途会跳一下。
            content.colored_label(
                if theme::smartisan::active() {
                    text_color
                } else {
                    theme::accent().lerp_to_gamma(selected_text, sel_t)
                },
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
        let label_width = (content.available_width() - CLOSE_HIT - CLOSE_GAP).max(24.0);
        let label_text = egui::RichText::new(truncate_middle(&title, self.tab_title_chars(tab)))
            .color(text_color);
        let label_response = content.add_sized(
            [label_width, TOOLBAR_CONTROL_HEIGHT - 8.0],
            egui::Label::new(label_text)
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
        let close_rect = close_button_rect(rect);
        // 自己 interact 再自己画，不走 Button：Button 的高度要跟行带的
        // interact_size 较劲（见 6bad39e），而这里的位置必须钉死；这样热区、
        // 悬停圆底、叉三者共用同一个 rect，不可能再错位。
        let close_response = content
            .interact(
                close_rect,
                egui::Id::unique("tab_close").with(tab),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("关闭这个标签");
        let close_hover_t = ctx.animate_bool_with_time(
            egui::Id::unique("tab_close_hover").with(tab),
            close_response.hovered(),
            TAB_HOVER_ANIM,
        );
        // 常态低对比**常显**（不悬停也看得见），悬停标签时升到实色、悬停它
        // 本身时渐变到危险红——三个档位各自明确。
        let close_base = theme::text_muted()
            .lerp_to_gamma(
                if rectangular {
                    theme::text()
                } else {
                    theme::accent_text()
                },
                sel_t,
            )
            .gamma_multiply(0.55 + 0.45 * hover_t);
        // 悬停关闭键的目标色：未选中是危险红；选中态在主题色胶囊上改用浅色叉
        // （红叉压在主题色上看不清）。两者同样按 sel_t 插值，避免中途突变。
        let close_hover_color = if rectangular {
            theme::danger()
        } else {
            theme::danger().lerp_to_gamma(theme::canvas(), sel_t)
        };
        let close_base = if theme::smartisan::active() {
            (if selected {
                theme::text_muted()
            } else {
                theme::smartisan::chrome_ink()
            })
            .gamma_multiply(0.65 + 0.25 * hover_t)
        } else {
            close_base
        };
        let close_color = close_base.lerp_to_gamma(close_hover_color, close_hover_t);
        // 悬停时叉底下垫一层同色淡圆，按下再深一档：热区变得看得见，也补上了
        // 「按下去了」的反馈。半透明叠在已画好的标签底上，选中态的主题色胶囊
        // 不会被糊成一块死色。
        let close_press = ctx.animate_bool_with_time(
            egui::Id::unique("tab_close_press").with(tab),
            close_response.is_pointer_button_down_on(),
            TAB_PRESS_ANIM,
        );
        let close_wash = theme::danger()
            .lerp_to_gamma(theme::accent_text(), sel_t)
            .gamma_multiply((0.16 + 0.12 * close_press) * close_hover_t);
        if rectangular {
            content.painter().rect_filled(
                close_rect,
                if theme::smartisan::active() { 3 } else { 0 },
                close_wash,
            );
        } else {
            content
                .painter()
                .circle_filled(close_rect.center(), CLOSE_HIT / 2.0, close_wash);
        }
        // 叉用 Lucide 的 x.svg：与工具栏其余图标同一套 2px 圆头描边，收口
        // 干净、两笔等长，不像系统字体的 × 那样随字体换形、还会被基线拽偏。
        theme::Icon::X
            .image_sized(CLOSE_ICON)
            .tint(close_color)
            .paint_at(
                &content,
                egui::Rect::from_center_size(close_rect.center(), egui::Vec2::splat(CLOSE_ICON)),
            );
        if close_response.clicked() {
            closed = true;
        }

        (clicked, closed, rect)
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
            let show_doc_controls = self.showing_doc();
            // 输入法状态：常显的一小块，点一下切中英。
            let ime_active = self.ime.active();
            let ime_english = self.ime.english();
            let ime_scheme = ime_scheme_marker(&self.config.ime);
            let active_doc = self.active_doc;
            let statistics = if show_doc_controls {
                let doc = &mut self.docs[active_doc];
                Some(doc.statistics.update(
                    &doc.draft,
                    &doc.generated_markdown,
                    &self.config,
                    ui.ctx(),
                ))
            } else {
                None
            };
            let statistics_width = statistics.as_ref().map_or(0.0, |(label, _)| {
                ui.painter()
                    .layout_no_wrap(
                        label.clone(),
                        egui::FontId::proportional(theme::font_sizes::SMALL),
                        theme::text_soft(),
                    )
                    .size()
                    .x
                    + 12.0
            });
            // 两个「显示」入口各自只在讲得通的模式里出场：源码/分栏给导航与行号，
            // 版式/分栏给倍率与纸面项。版本对照两边都不适用，状态栏保持原样。
            let (source_nav_visible, zoom_visible) = if show_doc_controls {
                let mode = self.docs[active_doc].preview_mode;
                (
                    matches!(mode, PreviewMode::Source | PreviewMode::Split),
                    matches!(mode, PreviewMode::Rendered | PreviewMode::Split),
                )
            } else {
                (false, false)
            };
            let (
                timeline_active,
                result_open,
                warnings_count,
                saved,
                candidates_open,
                candidates_count,
                candidates_visible,
            ) = if show_doc_controls {
                self.docs
                    .get(active_doc)
                    .map(|doc| {
                        let count = doc.candidates.items.len();
                        let visible = count > 0
                            && matches!(doc.preview_mode, PreviewMode::Source | PreviewMode::Split);
                        (
                            doc.preview_mode == PreviewMode::VersionDiff
                                && doc.draft_diff.timeline.expanded,
                            doc.result_drawer_open,
                            doc.warnings.len() + doc.revisions.pending_count(),
                            doc.manuscript_id.is_some(),
                            doc.candidates.open,
                            count,
                            visible,
                        )
                    })
                    .unwrap_or((false, false, 0, false, false, 0, false))
            } else {
                (false, false, 0, false, false, 0, false)
            };

            // 先给稿件统计和右侧入口留位；窗口变窄时状态文案缩短，模型名自动让位。
            let reserved = statistics_width
                + if show_doc_controls { 64.0 } else { 0.0 }
                + if ime_active { 96.0 } else { 0.0 }
                + if candidates_visible { 36.0 } else { 0.0 }
                + if source_nav_visible { 30.0 } else { 0.0 }
                + if zoom_visible { 58.0 } else { 0.0 }
                + 32.0;
            let status_limit = (ui.available_width() * 0.38)
                .clamp(160.0, 420.0)
                .min((ui.available_width() - reserved).max(0.0));
            ui.horizontal(|ui| {
                // 行高写死：`set_height` 同时钉住上下限，行内控件才不会去填满面板高度。
                ui.set_height(ROW_HEIGHT);

                // 左：忙碌指示灯与状态文案。文案过长直接截断，不许把中间的模型名挤走。
                if self.any_busy() {
                    // 与右边的常态圆点同样吃 8px：忙/闲切换时后面的状态文案
                    // 不会横向跳一下。
                    theme::spinner(ui, 8.0, theme::accent());
                } else {
                    theme::dot(ui, theme::success());
                }
                ui.add_space(2.0);
                ui.add_sized(
                    [status_limit, ROW_HEIGHT],
                    egui::Label::new(egui::RichText::new(status).color(theme::text_soft()))
                        .truncate(),
                );
                #[cfg(feature = "ai-workbench")]
                let left_bound = ui.min_rect().right();

                // 右：导出、审校、候选、版本四个抽屉入口。只留图标，说明放在悬停里。
                let _right_bound = ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // 输入法指示放最右：它是「现在在中文还是英文」唯一的常显提示，
                        // 不该只在打开稿件时才出现。
                        if ime_active {
                            if ime_chip(ui, ime_english, ime_scheme.as_deref(), CHIP_HEIGHT)
                                .clicked()
                            {
                                self.ime.toggle_english();
                            }
                            ui.add_space(4.0);
                        }
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
                            // 与 AI 侧栏同在右侧，同时开着中央区太窄。
                            if doc.result_drawer_open {
                                #[cfg(feature = "ai-workbench")]
                                {
                                    doc.ai_panel.open = false;
                                }
                            }
                        }
                        // 候选区入口：只在源码 / 分栏模式且已有条目时显示，与编辑区底
                        // 部的面板同步。图标方向随当前状态翻转，点一下切换展开。
                        if candidates_visible {
                            let candidate_icon = if candidates_open {
                                theme::Icon::PanelClose
                            } else {
                                theme::Icon::PanelOpen
                            };
                            let candidate_tip = if candidates_open {
                                format!("收起候选区（{candidates_count} 条）")
                            } else {
                                format!("展开候选区（{candidates_count} 条）")
                            };
                            if status_icon_button(
                                ui,
                                candidates_open,
                                candidate_icon,
                                &candidate_tip,
                                None,
                                None,
                            )
                            .clicked()
                            {
                                let doc = &mut self.docs[active_doc];
                                doc.candidates.open = !doc.candidates.open;
                            }
                        }
                        let version_tip = if saved {
                            "版本历史".to_string()
                        } else {
                            "版本历史（先保存）".to_string()
                        };
                        if status_icon_button(
                            ui,
                            timeline_active,
                            theme::Icon::History,
                            &version_tip,
                            None,
                            None,
                        )
                        .clicked()
                        {
                            // 第 ④ 期：不再开右侧抽屉，改为进入版本对照模式并展开左侧时间轴。
                            let doc = &mut self.docs[active_doc];
                            doc.preview_mode = PreviewMode::VersionDiff;
                            doc.draft_diff.timeline.expanded = true;
                        }
                        if zoom_visible {
                            self.status_zoom_menu(ui, active_doc);
                        }
                        if source_nav_visible {
                            self.status_source_nav_menu(ui, active_doc);
                        }
                        if let Some((label, tip)) = &statistics {
                            ui.add_space(8.0);
                            let response = ui.add_sized(
                                [
                                    (statistics_width - 12.0).min(ui.available_width()).max(0.0),
                                    ROW_HEIGHT,
                                ],
                                egui::Label::new(
                                    egui::RichText::new(label).color(theme::text_soft()),
                                )
                                .truncate()
                                .sense(egui::Sense::click()),
                            );
                            if response
                                .on_hover_text(tip)
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                // 页数不自动算：点这行才起一次排版。
                                self.docs[active_doc].statistics.request_pages();
                                ui.ctx().request_repaint();
                            }
                        }
                        ui.min_rect().left()
                    })
                    .inner;

                // 中：写作模型状态。画在左右两组之间的空档里，不额外占纵向空间。
                #[cfg(feature = "ai-workbench")]
                {
                    let gap_left = left_bound + 10.0;
                    let gap_right = _right_bound - 10.0;
                    if gap_right - gap_left > 80.0 {
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
                            |ui| self.writing_model_chip(ui),
                        );
                    }
                }
            });
        });
    }

    /// 显示类开关改完立即落盘，与功能区里同名的开关一致：两处读写同一份配置。
    fn persist_view_prefs(&mut self) {
        let _ = crate::storage::save(&self.config);
    }

    /// 状态栏右端的「倍率」入口：百分比常显，点开选档位或切纸面显示项。
    ///
    /// 「视图」分区退役后，缩放、纸面行号与导航刻度由这里接管。它们回答的是
    /// 「这一篇现在怎么看」，属于状态；常显百分比还顺带说明「当前不是 100%」。
    fn status_zoom_menu(&mut self, ui: &mut egui::Ui, doc: usize) {
        let zoom = self.docs[doc].preview_zoom;
        let current = zoom.unwrap_or(self.docs[doc].preview_fit_scale);
        let chip = status_menu_button(
            false,
            egui::Button::new(egui::RichText::new(format!("{:.0}%", current * 100.0)))
                .frame_when_inactive(false)
                .min_size(egui::vec2(38.0, 20.0)),
        );
        let _ = egui::containers::menu::MenuButton::from_button(chip)
            .ui(ui, |ui| {
                compact_menu(ui);
                ui.set_min_width(168.0);
                ui.label(egui::RichText::new("缩放").color(theme::text_muted()));
                let fit_selected = zoom.is_none();
                if ui
                    .add(
                        theme::menu_item(theme::Icon::FitWidth, "适应宽度")
                            .selected(fit_selected)
                            .frame_when_inactive(fit_selected),
                    )
                    .on_hover_text("回到按窗格宽度自动缩放")
                    .clicked()
                {
                    self.docs[doc].preview_zoom = None;
                    ui.close();
                }
                for step in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
                    let selected = zoom.is_some_and(|zoom| (zoom - step).abs() < 0.001);
                    if ui
                        .add(theme::menu_selectable_item(
                            selected,
                            &format!("{:.0}%", step * 100.0),
                        ))
                        .clicked()
                    {
                        self.docs[doc].preview_zoom = Some(step);
                        ui.close();
                    }
                }
                ui.separator();
                ui.label(egui::RichText::new("版式").color(theme::text_muted()));
                let mut paper_lines = self.config.show_preview_line_numbers;
                if ui
                    .checkbox(&mut paper_lines, "纸面行号")
                    .on_hover_text(
                        "投屏对稿用：公文预览的左页边逐行标号，点一下号码把光标带到那一行",
                    )
                    .changed()
                {
                    self.config.show_preview_line_numbers = paper_lines;
                    self.persist_view_prefs();
                }
                let mut navigator = self.config.show_preview_navigator;
                if ui
                    .checkbox(&mut navigator, "导航刻度")
                    .on_hover_text("版式右缘按标题显示刻度；鼠标靠近右缘展开标题列表")
                    .changed()
                {
                    self.config.show_preview_navigator = navigator;
                    self.persist_view_prefs();
                }
            })
            .0
            .on_hover_text("预览倍率与纸面显示项");
    }

    /// 状态栏右端的「源码显示」入口：目录、缩略图与编辑区行号。
    ///
    /// 目录和缩略图只画在 Markdown 模式（对照模式左栏是编辑器、没有这两列），
    /// 所以在对照模式下把它们灰掉并说明原因，而不是让用户点了没反应。
    fn status_source_nav_menu(&mut self, ui: &mut egui::Ui, doc: usize) {
        let source_only = self.docs[doc].preview_mode == PreviewMode::Source;
        let outline = self.config.show_source_outline;
        let minimap = self.config.show_source_minimap;
        let button = status_menu_button(
            outline || minimap,
            egui::Button::image(theme::Icon::List.image())
                .image_tint_follows_text_color(true)
                .min_size(egui::vec2(26.0, 20.0)),
        );
        let _ = egui::containers::menu::MenuButton::from_button(button)
            .ui(ui, |ui| {
                compact_menu(ui);
                ui.set_min_width(168.0);
                ui.label(egui::RichText::new("源码显示").color(theme::text_muted()));
                let mut outline = outline;
                let mut minimap = minimap;
                ui.add_enabled_ui(source_only, |ui| {
                    if ui
                        .checkbox(&mut outline, "目录")
                        .on_hover_text("源码左侧的标题目录，点标题跳转")
                        .on_disabled_hover_text("目录只画在 Markdown 模式里")
                        .changed()
                    {
                        self.config.show_source_outline = outline;
                        self.persist_view_prefs();
                    }
                    if ui
                        .checkbox(&mut minimap, "缩略图")
                        .on_hover_text("源码右侧的全文缩略图，滚轮浏览，点击或拖动定位")
                        .on_disabled_hover_text("缩略图只画在 Markdown 模式里")
                        .changed()
                    {
                        self.config.show_source_minimap = minimap;
                        self.persist_view_prefs();
                    }
                });
                let mut line_numbers = self.config.show_editor_line_numbers;
                if ui
                    .checkbox(&mut line_numbers, "编辑区行号")
                    .on_hover_text("在 Markdown 源码编辑器左侧显示源码行号")
                    .changed()
                {
                    self.config.show_editor_line_numbers = line_numbers;
                    self.persist_view_prefs();
                }
            })
            .0
            .on_hover_text("源码模式的目录、缩略图与行号");
    }
}

/// 一条顶栏的横向分区（窗口坐标，逻辑像素）。
#[derive(Debug, Clone, Copy)]
struct RailZones {
    /// 菜单按钮左沿。
    menu_left: f32,
    /// 标签条：高度已经收成胶囊高，竖直居中。
    tabs: egui::Rect,
    /// 检索 / 换肤两枚工具。
    tools: egui::Rect,
    /// 工具与窗口按钮之间的分隔线；macOS 没有窗口按钮，也就没有这条线。
    divider: Option<egui::Rect>,
    /// 窗口按钮区左沿；macOS 用不上。
    controls_left: f32,
}

/// 按窗口矩形算顶栏各段的位置。
///
/// 抽成纯函数是为了脱开 egui 上下文测两条硬约束：标签永远不许压到右侧工具，
/// 并且两者之间至少留出 [`RAIL_DRAG_MIN`] 的拖拽带——窗口再窄也要拖得动。
fn rail_zones(rect: egui::Rect, native_left: f32, window_buttons: usize) -> RailZones {
    let left = rect.left() + native_left;
    let controls_left = rect.right() - window_buttons as f32 * RAIL_CONTROL_WIDTH;
    let (tools_right, divider) = if window_buttons == 0 {
        // macOS：红黄绿灯在左，右侧没有窗口按钮，工具组自己收边。
        (rect.right() - 12.0, None)
    } else {
        let divider_x = controls_left - 11.0;
        (
            controls_left - RAIL_CONTROLS_GAP,
            Some(egui::Rect::from_min_size(
                egui::pos2(divider_x, rect.center().y - RAIL_DIVIDER_HEIGHT / 2.0),
                egui::vec2(1.0, RAIL_DIVIDER_HEIGHT),
            )),
        )
    };
    let tools = egui::Rect::from_min_max(
        egui::pos2(tools_right - RAIL_TOOLS_WIDTH, rect.top()),
        egui::pos2(tools_right, rect.bottom()),
    );
    let tabs_left = left + RAIL_LEFT_WIDTH;
    let tabs_right = (tools.left() - RAIL_DRAG_MIN).max(tabs_left);
    let half = TOOLBAR_CONTROL_HEIGHT / 2.0;
    RailZones {
        menu_left: left + 10.0,
        tabs: egui::Rect::from_min_max(
            egui::pos2(tabs_left, rect.center().y - half),
            egui::pos2(tabs_right, rect.center().y + half),
        ),
        tools,
        divider,
        controls_left,
    }
}

/// 品牌标压在左侧留白里，小箭头贴右沿：两者合起来是一枚 40 px 宽的按钮。
fn paint_menu_button(ui: &egui::Ui, rect: egui::Rect, mark: egui::Color32, chevron: egui::Color32) {
    theme::brand_image(18.0, mark).paint_at(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 13.0, rect.center().y),
            egui::Vec2::splat(18.0),
        ),
    );
    theme::Icon::ChevronDown
        .image_sized(9.0)
        .tint(chevron)
        .paint_at(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 31.0, rect.center().y + 0.5),
                egui::Vec2::splat(9.0),
            ),
        );
}

/// 窗口按钮：传统桌面三枚（46 px 一格、满高），Hyprland 只有关闭。
fn chrome_window_buttons(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    zones: RailZones,
    buttons: &[TitlebarBtn],
    maximized: bool,
    focused: bool,
) {
    let ctx = ui.ctx().clone();
    let title_color = if theme::smartisan::active() {
        theme::smartisan::chrome_ink().gamma_multiply(if focused { 1.0 } else { 0.75 })
    } else if focused {
        theme::text_soft()
    } else {
        theme::text_muted()
    };

    let mut action: Option<TitlebarAction> = None;
    for (index, kind) in buttons.iter().copied().enumerate() {
        let btn_rect = egui::Rect::from_min_size(
            egui::pos2(
                zones.controls_left + index as f32 * RAIL_CONTROL_WIDTH,
                rect.top(),
            ),
            egui::vec2(RAIL_CONTROL_WIDTH, rect.height()),
        );
        let response = ui
            .interact(
                btn_rect,
                ui.scope_id().with(("titlebar_btn", index)),
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
        // 关闭键悬停用白字（Windows 惯例），其余按钮保持文字色。
        let icon_color = if is_close && hovered {
            theme::accent_text()
        } else if hovered && theme::smartisan::active() {
            theme::text()
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

#[cfg(test)]
mod rail_tests {
    use super::*;

    fn rail(width: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, RAIL_HEIGHT))
    }

    #[test]
    fn the_tabs_start_after_a_fixed_left_cluster() {
        let zones = rail_zones(rail(1560.0), 0.0, 3);
        assert!((zones.tabs.left() - RAIL_LEFT_WIDTH).abs() < f32::EPSILON);
        assert!(
            (zones.controls_left - (1560.0 - 3.0 * RAIL_CONTROL_WIDTH)).abs() < f32::EPSILON,
            "窗口按钮从右沿往前排"
        );
    }

    #[test]
    fn the_tabs_always_leave_a_drag_band_before_the_tools() {
        for width in [760.0, 900.0, 1280.0, 1560.0, 2560.0] {
            let zones = rail_zones(rail(width), 0.0, 3);
            assert!(
                zones.tabs.right() <= zones.tools.left() - RAIL_DRAG_MIN + 0.01,
                "{width} px 宽时标签压到了工具"
            );
            assert!(
                zones.tabs.width() >= DOC_TAB_MIN_WIDTH,
                "{width} px 宽时标签不够一格"
            );
        }
    }

    #[test]
    fn the_window_buttons_keep_their_distance() {
        let zones = rail_zones(rail(1560.0), 0.0, 3);
        let divider = zones.divider.expect("有窗口按钮就该有分隔线");
        assert!(zones.tools.right() <= divider.left());
        assert!(divider.right() + 9.0 <= zones.controls_left);
    }

    #[test]
    fn macos_leaves_room_for_the_traffic_lights() {
        let zones = rail_zones(rail(1560.0), 70.0, 0);
        assert!((zones.tabs.left() - (70.0 + RAIL_LEFT_WIDTH)).abs() < f32::EPSILON);
        assert!(zones.divider.is_none());
        assert!((zones.tools.right() - (1560.0 - 12.0)).abs() < f32::EPSILON);
    }
}

#[cfg(test)]
mod tab_close_tests {
    use super::*;

    /// 一格标签：最窄的那种，右端最容易被挤。
    fn tab() -> egui::Rect {
        egui::Rect::from_min_size(
            egui::pos2(120.0, 6.0),
            egui::vec2(DOC_TAB_MIN_WIDTH, TOOLBAR_CONTROL_HEIGHT),
        )
    }

    #[test]
    fn the_close_button_sits_on_the_tabs_own_centre_line() {
        let tab = tab();
        let close = close_button_rect(tab);
        // 竖直方向对齐胶囊中线，不是贴着下沿——这正是之前跑偏的那一次。
        assert!((close.center().y - tab.center().y).abs() < f32::EPSILON);
        // 上下都留得出余量，底圆不会压到胶囊描边。
        assert!(close.top() > tab.top() + 2.0);
        assert!(close.bottom() < tab.bottom() - 2.0);
    }

    #[test]
    fn the_close_button_keeps_its_margin_from_the_right_edge() {
        let close = close_button_rect(tab());
        assert!((close.right() - (tab().right() - CLOSE_MARGIN)).abs() < f32::EPSILON);
        assert!((close.width() - CLOSE_HIT).abs() < f32::EPSILON);
        assert!((close.height() - CLOSE_HIT).abs() < f32::EPSILON);
    }

    #[test]
    fn the_cross_stays_inside_its_hit_area() {
        let close = close_button_rect(tab());
        let icon = egui::Rect::from_center_size(close.center(), egui::Vec2::splat(CLOSE_ICON));
        assert!(close.contains_rect(icon));
        // 叉小于热区：热区大一圈才好点，而不是让图标自己撑满。
        const { assert!(CLOSE_ICON < CLOSE_HIT) };
    }

    /// 标签宽度里给关闭键预留的位置必须够：热区 + 两侧留白 + 标题前的间隙，
    /// 否则长标题的省略号会压到叉上。
    #[test]
    fn the_tab_width_budget_covers_the_close_button() {
        let content_padding = CLOSE_MARGIN * 2.0;
        assert!(DOC_TAB_CHROME_WIDTH >= content_padding + CLOSE_HIT + CLOSE_GAP);
    }
}
