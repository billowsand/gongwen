//! 按钮、菜单、分隔线与通用控件。
//!
//! 由 src/theme.rs 拆分而来：本文件是模块 `theme::widgets`，与其它子模块共享
//! `theme` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::theme::{
    Icon, Theme, accent, accent_active, accent_hover, border, border_strong, chrome_radius,
    current, danger, font_sizes, is_mdex, mdex, smartisan, surface, surface_active, surface_hover,
    surface_sunk, text, text_muted, text_soft, warn,
};
use eframe::egui;
use eframe::egui::{Color32, CornerRadius, Margin, Stroke};

/// 图标与文字组合的普通按钮。
pub(crate) fn icon_text_button(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(icon.image(), label.to_owned())
        .image_tint_follows_text_color(true)
        .corner_radius(chrome_radius(7))
}

/// 菜单中的图文条目。保持按钮 frame 开启，常态仍由 egui 的 menu style 画成
/// 透明底，但悬停/按下时会铺满整行背景；显式 `.frame(false)` 会连悬停反馈一起
/// 关掉，菜单看起来像普通文字，也会把可点击范围收窄到内容附近。
pub(crate) fn menu_item(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(icon.image(), label.to_owned())
        .image_tint_follows_text_color(true)
        .corner_radius(chrome_radius(5))
        .min_size(egui::vec2(0.0, 26.0))
}

/// 菜单中的纯文字条目，与图文条目使用同一高度和悬停反馈。
pub(crate) fn menu_text_item(label: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(label.into())
        .corner_radius(chrome_radius(5))
        .min_size(egui::vec2(0.0, 26.0))
}

/// 菜单中的单选条目。选中项常显淡底，未选中项只在悬停/按下时显示背景。
pub(crate) fn menu_selectable_item(selected: bool, label: &str) -> egui::Button<'static> {
    menu_text_item(label)
        .selected(selected)
        .frame_when_inactive(selected)
}

/// 浮层（下拉菜单、弹层、模态框、不可拖大小的浮窗）里的竖向滚动列表：最高
/// `max_height`，内容少时照常按内容收缩。
///
/// egui 的浮层按**上一帧**的尺寸给内容留地方，滚动区取「剩下的地方」与 `max_height`
/// 的较小者。内容一变少（筛选、切组），浮层跟着变矮；下一帧滚动区只剩矮的那么高，
/// 内容再多也长不回去，只能在一小截里滚。把最小滚动高度也设成 `max_height`，滚动区
/// 就按 `max_height` 排、自己把浮层撑开，内容不足时仍收缩到内容高度。
///
/// 只用在浮层里：嵌在面板里的滚动区可用高度是实打实的，强撑到 `max_height` 会冒出面板。
pub(crate) fn popup_scroll(max_height: f32) -> egui::ScrollArea {
    egui::ScrollArea::vertical()
        .max_height(max_height)
        .min_scrolled_height(max_height)
}

/// 仿 ComboBox 的下拉按钮：选中项靠左、过长截断，右端一个向下的箭头。配
/// `egui::Popup::menu` 用。
///
/// 弹层内容会变（顶上有筛选框）的下拉用它，不用 `egui::ComboBox`：ComboBox 把内容
/// 包进自己的滚动区，那个滚动区同样会被上一帧的弹层高度卡住（见 [`popup_scroll`]），
/// 而且从外面改不了它。
pub(crate) fn dropdown_button(text: impl Into<String>, width: f32) -> egui::Button<'static> {
    egui::Button::new(text.into())
        .right_text(Icon::ChevronDown.image_sized(12.0))
        .image_tint_follows_text_color(true)
        .truncate()
        .min_size(egui::vec2(width, 0.0))
}

/// 强调色实底上的文字与图标。夜墨的浅绿底使用墨色，避免白字失去对比度。
pub(crate) fn accent_text() -> Color32 {
    accent_text_for(current())
}

pub(crate) fn accent_text_for(palette: Theme) -> Color32 {
    if palette.label == mdex::DARK_LABEL {
        Color32::from_rgb(0x19, 0x1C, 0x19)
    } else {
        Color32::WHITE
    }
}

/// 在子作用域内把按钮三态底色覆盖成主题强调色（clone-on-write，退出自动还原），
/// 让 `egui::Button` 自己按状态切换底色并保留按压动效。供主按钮与需要自定义
/// 尺寸的橙色按钮共用。
pub(crate) fn accent_scope<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = accent();
        widgets.inactive.bg_fill = accent();
        widgets.inactive.fg_stroke = Stroke::new(1.0, accent_text());
        widgets.hovered.weak_bg_fill = accent_hover();
        widgets.hovered.bg_fill = accent_hover();
        widgets.hovered.fg_stroke = Stroke::new(1.5, accent_text());
        widgets.active.weak_bg_fill = accent_active();
        widgets.active.bg_fill = accent_active();
        widgets.active.fg_stroke = Stroke::new(1.5, accent_text());
        add_contents(ui)
    })
    .inner
}

/// 主按钮内部的图文 Button（自适应字色、强调色描边），不含底色——底色由 `accent_scope` 提供。
/// 需要自定义尺寸（如 `add_sized`）时，在 `accent_scope` 里 add 这个 widget。
pub(crate) fn primary_button_widget(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(
        icon.image().tint(accent_text()),
        egui::RichText::new(label.to_owned())
            .color(accent_text())
            .strong(),
    )
    .stroke(Stroke::new(1.0, accent_active()))
    .corner_radius(chrome_radius(7))
}

/// 图标与文字组合的主按钮。
///
/// 不能用 `.fill(ACCENT)` 写死背景：egui 的 `Button::fill` 会覆盖所有交互状态
/// （normal/hovered/active）的底色，按压时背景不变，看起来「像没点中」。这里改为
/// 用 `accent_scope` 按状态切换底色，保留按压动效。
pub(crate) fn primary_icon_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    primary_icon_button_enabled(ui, true, icon, label)
}

/// 可禁用的主按钮。禁用时退化为灰底，不参与橙色状态切换。
pub(crate) fn primary_icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    if !enabled {
        return ui.add_enabled(false, primary_button_widget(icon, label).fill(text_muted()));
    }
    accent_scope(ui, |ui| ui.add(primary_button_widget(icon, label)))
}

/// 图标与文字组合的次按钮。
pub(crate) fn secondary_icon_button(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(icon.image(), label.to_owned())
        .image_tint_follows_text_color(true)
        .fill(surface())
        .stroke(Stroke::new(1.0, border_strong()))
        .corner_radius(chrome_radius(7))
}

/// 图标与文字组合的警示按钮，用于需要保留明确文字的删除/清空操作。
pub(crate) fn warning_icon_button(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(
        icon.image().tint(warn()),
        egui::RichText::new(label.to_owned()).color(warn()),
    )
    .corner_radius(chrome_radius(7))
}

/// 已选项标签：文字在左，移除图标固定在右；未知词条沿用警示色。
pub(crate) fn removable_tag_button(label: &str, warning: bool) -> egui::Button<'static> {
    let color = if warning { warn() } else { text_soft() };
    egui::Button::new(egui::RichText::new(label.to_owned()).color(color))
        .right_text(Icon::X.image())
        .image_tint_follows_text_color(true)
        .small()
}

/// 只显示图形的紧凑按钮。`label` 同时用作悬停说明与无障碍名称。
pub(crate) fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    icon_button_impl(ui, true, icon, label, false)
}

/// 可禁用的图标按钮。
pub(crate) fn icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    icon_button_impl(ui, enabled, icon, label, false)
}

/// 危险操作图标按钮。仅改变图标颜色，不用大面积红底抢夺视觉注意力。
pub(crate) fn danger_icon_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    icon_button_impl(ui, true, icon, label, true)
}

/// 顶部导航按钮：保留文字识别效率，同时用图标建立稳定的视觉锚点。
pub(crate) fn nav_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    ui.add(
        egui::Button::image_and_text(icon.image(), label)
            .image_tint_follows_text_color(true)
            .selected(selected)
            .frame_when_inactive(selected)
            .min_size(egui::vec2(74.0, 28.0)),
    )
}

/// 功能区分区卡：选中项的底色属于外层闭合 Ribbon 轮廓，这里只负责文字与交互，
/// 避免按钮自己再画一个矩形，把连接曲线切断。
pub(crate) fn ribbon_tab_button(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    compact: bool,
) -> egui::Response {
    let text = if selected {
        egui::RichText::new(label).color(accent()).strong()
    } else {
        egui::RichText::new(label).color(text_soft())
    };
    let button = egui::Button::new(text)
        .corner_radius(chrome_radius(6))
        .min_size(egui::vec2(if compact { 38.0 } else { 52.0 }, 24.0));
    ui.add(if selected {
        button.frame(false)
    } else {
        // 未选中项常态透明，悬停时仍保留按钮反馈。
        button.frame_when_inactive(false)
    })
}

/// 编辑区右上角的视图切换：仿代码编辑器只保留图标，名称放在悬停说明中。
pub(crate) fn view_icon_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    ui.add(
        egui::Button::image(icon.image())
            .image_tint_follows_text_color(true)
            .selected(selected)
            .frame_when_inactive(selected)
            .min_size(egui::vec2(30.0, 28.0))
            .corner_radius(chrome_radius(6)),
    )
    .on_hover_text(label)
}

pub(crate) fn icon_button_impl(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: &str,
    dangerous: bool,
) -> egui::Response {
    let image = if dangerous {
        icon.image()
            .tint(if enabled { danger() } else { text_muted() })
    } else {
        icon.image()
    };
    let response = ui.add_enabled(
        enabled,
        egui::Button::image(image)
            .image_tint_follows_text_color(!dangerous)
            .min_size(egui::vec2(28.0, 26.0))
            .corner_radius(chrome_radius(6)),
    );
    if smartisan::active() {
        smartisan::bevel(
            ui.painter(),
            response.rect,
            response.is_pointer_button_down_on(),
            false,
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response.on_hover_text(label)
}

/// 标题栏快速访问的图标按钮：常态无底色、悬停才铺浅底，仿 Word 快速访问工具栏。
///
/// 自己分配固定的 26×24 而不走 `egui::Button`：后者的高度下限是全局
/// `interact_size.y`（30 px），塞进 34 px 高的标题栏会上下各溢出、压到下沿分隔线。
pub(crate) fn titlebar_icon_button(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 24.0), sense);
    if ui.is_rect_visible(rect) {
        if smartisan::active() {
            let pressed = response.is_pointer_button_down_on();
            let top = if pressed {
                Color32::from_rgb(105, 88, 72)
            } else {
                Color32::from_rgb(132, 114, 96)
            };
            let bottom = if pressed {
                Color32::from_rgb(118, 99, 82)
            } else {
                Color32::from_rgb(119, 101, 85)
            };
            let hover = enabled && response.hovered() && !pressed;
            let top = if hover {
                top.lerp_to_gamma(smartisan::chrome_ink(), 0.06)
            } else {
                top
            };
            let bottom = if hover {
                bottom.lerp_to_gamma(smartisan::chrome_ink(), 0.06)
            } else {
                bottom
            };
            smartisan::rounded_gradient(ui.painter(), rect.shrink(1.0), 4.0, top, bottom);
            smartisan::chrome_edge(ui.painter(), rect, pressed);
            icon.image()
                .tint(smartisan::chrome_ink().gamma_multiply(if enabled { 1.0 } else { 0.4 }))
                .paint_at(
                    ui,
                    egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)),
                );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label)
            });
            return response.on_hover_text(label);
        }
        let fill = if !enabled {
            Color32::TRANSPARENT
        } else if response.is_pointer_button_down_on() {
            surface_active()
        } else if response.hovered() {
            surface_hover()
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, chrome_radius(5), fill);
        }
        let tint = if !enabled {
            text_muted()
        } else if response.hovered() {
            text()
        } else {
            text_soft()
        };
        icon.image().tint(tint).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)),
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response.on_hover_text(label)
}

/// 状态小圆点，用于状态栏与列表行。
pub(crate) fn dot(ui: &mut egui::Ui, color: Color32) {
    let size = egui::vec2(8.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// 行内忙碌指示：一段匀速旋转、尾部渐隐的圆弧，尺寸自己说了算。
///
/// 不用 egui 自带的 `Spinner`：它的尺寸取 `spacing.interact_size.y`（本主题是
/// 30px），塞进标签页内容行（28-8=20px）这类窄行会把行撑高——圆圈溢出胶囊、
/// 同一行的标题被重新居中而往下掉。这里显式吃 `size × size`，调用处按邻座图标
/// 的尺寸给值，忙与不忙之间布局纹丝不动。
///
/// 画法也比自带的稳：自带 spinner 的扫过角是 `sin(t)`，会一伸一缩地抽搐；这里
/// 扫过角固定 280°，匀速转，尾巴用逐段降透明度渐隐，头尾各补一枚圆点当圆头。
pub(crate) fn spinner(ui: &mut egui::Ui, size: f32, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    // 线宽随直径走，小到 14px 也保持 1.5px 的可见度。
    let width = (size / 8.0).clamp(1.5, 3.0);
    let radius = (size - width) / 2.0;
    /// 转一圈的秒数。
    const PERIOD: f64 = 0.9;
    /// 圆弧扫过的角度：留一道明显缺口，一眼看出在转。
    const SWEEP: f64 = std::f64::consts::TAU * 280.0 / 360.0;
    /// 分段数：段越多尾巴的渐隐越顺，14px 上 24 段已看不出折线。
    const SEGMENTS: usize = 24;
    let head = ui.input(|i| i.time) % PERIOD / PERIOD * std::f64::consts::TAU;
    let at = |t: f64| {
        // t=0 是尾、t=1 是头，弧沿顺时针铺开。
        let (sin, cos) = (head - SWEEP * (1.0 - t)).sin_cos();
        rect.center() + radius * egui::vec2(cos as f32, sin as f32)
    };
    let painter = ui.painter();
    for i in 0..SEGMENTS {
        let t0 = i as f64 / SEGMENTS as f64;
        let t1 = (i + 1) as f64 / SEGMENTS as f64;
        // 尾端压到 0.15，头端实色：旋转方向不靠缺口猜，看浓淡就知道。
        let alpha = 0.15 + 0.85 * t1 as f32;
        painter.line_segment(
            [at(t0), at(t1)],
            Stroke::new(width, color.gamma_multiply(alpha)),
        );
    }
    // 线段是平头的，头尾各补一枚同宽圆点收口，弧线才不像被剪断。
    painter.circle_filled(at(1.0), width / 2.0, color);
    painter.circle_filled(at(0.0), width / 2.0, color.gamma_multiply(0.15));
    // 动画得自己驱动重绘，否则界面静止时它就停在那儿。
    ui.ctx().request_repaint();
    response
}

/// 一枚单行淡底圆角标签，用来显示模型名、状态、版本号等元信息。
pub(crate) fn chip(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) -> egui::Response {
    // 先按整行宽度排好单行文字，再交给外层布局整体换行。若在 Frame 内按
    // 行尾剩余宽度排字，模型名会被挤成竖排并撑高整张卡片。
    let galley = egui::WidgetText::from(egui::RichText::new(text).color(fg)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (ui.max_rect().width() - 16.0).max(0.0),
        egui::TextStyle::Body,
    );
    let truncated = galley.elided;
    // 文字与背景一起分配空间，避免 Frame 的子布局先在行尾落位。
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + egui::vec2(16.0, 4.0), egui::Sense::hover());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), text));
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, chrome_radius(255), bg);
        ui.painter()
            .galley(rect.min + egui::vec2(8.0, 2.0), galley, fg);
    }
    if truncated {
        response.on_hover_text(text)
    } else {
        response
    }
}

/// 单行输入框。
///
/// egui 默认给 `TextEdit` 的竖边距是 2pt，按钮和下拉框却按 `button_padding`
/// 留 5pt，同一行里输入框因此比旁边的按钮矮 4px，一排控件上下沿全对不齐。
/// 这里按按钮的内边距配平，凡是与按钮同行的输入框都从这里取。
pub(crate) fn field<'t>(
    text: &'t mut dyn egui::TextBuffer,
    hint: &str,
    width: f32,
) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text)
        .hint_text(hint.to_owned())
        .desired_width(width)
        .margin(Margin::symmetric(8, 4))
}

/// 控件前后的小字说明。次级文字统一走这一档字号与颜色，免得同一行里
/// 说明与正文一样重。
pub(crate) fn caption(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(
        egui::RichText::new(text)
            .size(font_sizes::SMALL)
            .color(text_muted()),
    )
}

/// 概况条里的一格：上行数值、下行说明。数字比正文大一档，扫一眼就能比出
/// 多少，不必读完整句话。
pub(crate) fn metric(
    ui: &mut egui::Ui,
    value: &str,
    label: &str,
    color: Color32,
) -> egui::Response {
    // 0 一律不着色：空表里一排红色、绿色的 0 看着像出了状况，其实什么也没发生。
    let color = if value.trim() == "0" {
        text_muted()
    } else {
        color
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(
            egui::RichText::new(value)
                .size(font_sizes::HEADING)
                .color(color)
                .strong(),
        );
        ui.label(
            egui::RichText::new(label)
                .size(font_sizes::SMALL)
                .color(text_muted()),
        );
    })
    .response
}

/// 行内竖分隔线，用来把概况条分组。
///
/// 不能用 `ui.separator()`：它在横向布局里会撑满整个可视高度，把下面的内容
/// 一并顶开。
pub(crate) fn divider_v(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, border());
}

/// 卡片内的横向细线，用于分开同一张卡里的两组内容。
pub(crate) fn hairline(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, border());
}

/// 说明条：淡底圆角 + 左侧色条 + 图标，用于整段的提醒与注意事项。
///
/// 成段的警示文字直接用 `colored_label` 铺出来，几条连排就是一片红字，用户会
/// 整片跳过。收进带色条的淡底块里，一眼能看出「这是一条提示」而不是正文。
pub(crate) fn notice(
    ui: &mut egui::Ui,
    icon: Icon,
    fg: Color32,
    bg: Color32,
    text: impl Into<String>,
) -> egui::Response {
    let response = egui::Frame::new()
        .fill(bg)
        .corner_radius(chrome_radius(7))
        .inner_margin(Margin {
            left: 12,
            right: 10,
            top: 6,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.add(icon.image_sized(14.0).tint(fg));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(text.into())
                            .color(fg)
                            .size(font_sizes::SMALL),
                    )
                    .wrap(),
                );
            });
        })
        .response;
    // 左侧色条只画左边两角的圆，右边贴着底色接上，看起来是从块里长出来的。
    let bar = egui::Rect::from_min_size(response.rect.min, egui::vec2(3.0, response.rect.height()));
    ui.painter().rect_filled(
        bar,
        if is_mdex() {
            CornerRadius::ZERO
        } else {
            CornerRadius {
                nw: 7,
                sw: 7,
                ne: 0,
                se: 0,
            }
        },
        fg,
    );
    response
}

/// 分段选择器：把一组互斥的选项收进同一枚浅底胶囊，和旁边零散的按钮区分开。
pub(crate) fn segmented<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(surface_sunk())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(chrome_radius(8))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().button_padding = egui::vec2(9.0, 4.0);
            ui.horizontal(|ui| add_contents(ui)).inner
        })
        .inner
}
