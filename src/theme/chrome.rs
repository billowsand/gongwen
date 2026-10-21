//! 卡片、面板、Ribbon 外形与窗口过场动效。
//!
//! 由 src/theme.rs 拆分而来：本文件是模块 `theme::chrome`，与其它子模块共享
//! `theme` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::theme::{
    accent, accent_soft, anim, border, border_strong, canvas, chrome_radius, is_mdex, smartisan,
    surface, surface_active, surface_hover,
};
use eframe::egui;
use eframe::egui::{Color32, Margin, Stroke};

/// 卡片外框：白纸面 + 细描边 + 圆角。
pub(crate) fn card() -> egui::Frame {
    if smartisan::active() {
        return egui::Frame::new()
            .fill(surface())
            .stroke(Stroke::new(1.0, border()))
            .corner_radius(3)
            .shadow(paper_shadow(18))
            .inner_margin(Margin::same(12));
    }
    egui::Frame::new()
        .fill(surface())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(chrome_radius(8))
        .inner_margin(Margin::same(10))
}

/// 可点击的卡片行（列表里“点一下选中/编辑”的条目）。
///
/// 整张卡片都是点击区：指针移上去变成手型，底色淡入悬停色、描边加深；按下时
/// 底色再压一档，松开回弹，手感和按钮一致。选中项在强调色淡底上做同样的深浅变化。
///
/// 点击感知挂在外层 `Ui` 上（`UiBuilder::sense`），它比子部件先注册，卡片里的
/// 复选框、小按钮仍优先拿到点击。卡片内关掉 `selectable_labels`：否则标签会把
/// 点击吞掉，指针还会变成文本 I 形光标。悬停用 `contains_pointer` 的几何判断，
/// 指针落在子部件上时整张卡片仍算悬停，底色不会一闪一闪。
///
/// `frame` 的底色与描边作为静止态；返回值的 `response` 是整张卡片的点击响应。
pub(crate) fn clickable_card<R>(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    frame: egui::Frame,
    selected: bool,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(id_salt)
            .sense(egui::Sense::click()),
        |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            // 取的是上一帧的矩形与交互状态，本帧据此决定底色，再画内容。
            let response = ui.response();
            let hovered = response.contains_pointer();
            let pressed = hovered && response.is_pointer_button_down_on();
            if hovered {
                // 先设手型，子部件（如输入框）若要自己的光标可以在后面覆盖。
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let id = ui.id();
            let hover_t = ui
                .ctx()
                .animate_bool_with_time(id.with("hover"), hovered, anim::FAST);
            let press_t = ui
                .ctx()
                .animate_bool_with_time(id.with("press"), pressed, anim::FAST);

            let (hover_fill, press_fill) = if selected {
                (
                    mix_color(accent_soft(), accent(), 0.06),
                    mix_color(accent_soft(), accent(), 0.14),
                )
            } else {
                (surface_hover(), surface_active())
            };
            let fill = frame
                .fill
                .lerp_to_gamma(hover_fill, hover_t)
                .lerp_to_gamma(press_fill, press_t);
            let mut stroke = frame.stroke;
            if stroke.width > 0.0 {
                let target = if selected { accent() } else { border_strong() };
                stroke.color = stroke.color.lerp_to_gamma(target, hover_t * 0.6);
            }

            let mut prepared = frame.fill(fill).stroke(stroke).begin(ui);
            let inner = add_contents(&mut prepared.content_ui);
            prepared.end(ui);
            inner
        },
    )
}

/// 常规面板的圆角。卡片、目录、缩略图、候选区与浮板都用它；纸面要像纸，是唯一的例外（3）。
pub const PANE_RADIUS: u8 = 8;

/// 面板与面板、面板与窗口边缘之间的缝。
pub const PANE_GAP: i8 = 6;

/// 面板内边距。
pub const PANE_PADDING: i8 = 8;

/// 浮起元素（浮板、纸面）统一的一档投影；`alpha` 由调用方按纸面明暗给。
pub(crate) fn float_shadow(alpha: u8) -> egui::epaint::Shadow {
    if is_mdex() {
        return egui::epaint::Shadow::NONE;
    }
    egui::epaint::Shadow {
        offset: [0, 3],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(alpha),
    }
}

/// 纸张保留轻微离底的投影；MDEX 的其它界面仍是平面工具区。
pub(crate) fn paper_shadow(alpha: u8) -> egui::epaint::Shadow {
    if is_mdex() {
        egui::epaint::Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(alpha.max(32)),
        }
    } else {
        float_shadow(alpha)
    }
}

/// 对照区不叠面板外框，仅用底色区分源码与纸张承托面。
pub(crate) fn comparison_pane(preview: bool) -> egui::Frame {
    if smartisan::active() {
        return egui::Frame::new().inner_margin(Margin::same(16));
    }
    if !is_mdex() {
        return pane();
    }
    egui::Frame::new()
        .fill(if preview { canvas() } else { surface() })
        .inner_margin(Margin::same(if preview { 16 } else { PANE_PADDING }))
}

/// 对照区的浮动滚动条不占内容宽度，空闲时隐藏，悬停时仍可抓住拖动。
pub(crate) fn configure_comparison_scroll(ui: &mut egui::Ui) {
    if is_mdex() {
        ui.spacing_mut().scroll = egui::style::ScrollStyle {
            bar_width: 6.0,
            floating_width: 3.0,
            active_background_opacity: 0.0,
            interact_background_opacity: 0.0,
            active_handle_opacity: 0.4,
            ..egui::style::ScrollStyle::floating()
        };
    }
}

/// 平铺面板外框：surface 底 + 细描边 + 统一圆角，不加阴影，四周留出 [`PANE_GAP`] 的缝。
pub(crate) fn pane() -> egui::Frame {
    egui::Frame::new()
        .fill(surface())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(chrome_radius(PANE_RADIUS))
        .inner_margin(Margin::same(PANE_PADDING))
        .outer_margin(Margin::same(PANE_GAP))
}

/// 面板外框：只填底色，不描边；`margin` 为内边距。
pub(crate) fn panel(fill: Color32, margin: i8) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(Margin::symmetric(margin, (margin / 2).max(4)))
}

/// 功能区第二行只负责留出内容边距；底色和轮廓等上下两行完成排版后统一绘制。
pub(crate) fn ribbon_tray_layout() -> egui::Frame {
    if smartisan::active() {
        return egui::Frame::new().inner_margin(Margin::symmetric(2, 4));
    }
    egui::Frame::new()
        .inner_margin(Margin::symmetric(7, 2))
        .outer_margin(Margin::symmetric(2, 2))
}

/// MDEX 用淡色工具带、底线与同色短标记关联上下两行；其它主题保持闭合曲线。
pub(crate) fn connected_ribbon_shape(
    tab_rect: egui::Rect,
    tray_outer_rect: egui::Rect,
) -> egui::Shape {
    if smartisan::active() {
        return smartisan::ribbon_shape(tab_rect, tray_outer_rect);
    }
    if is_mdex() {
        let tray = tray_outer_rect.shrink(2.0);
        let fill = mix_color(surface(), accent_soft(), 0.18);
        return egui::Shape::Vec(vec![
            egui::Shape::rect_filled(tray, 0, fill),
            egui::Shape::line_segment(
                [tray.left_bottom(), tray.right_bottom()],
                Stroke::new(1.0, border()),
            ),
            egui::Shape::line_segment(
                [
                    egui::pos2(tray.left() + 1.0, tray.top() + 6.0),
                    egui::pos2(tray.left() + 1.0, tray.bottom() - 6.0),
                ],
                Stroke::new(2.0, accent()),
            ),
            egui::Shape::rect_filled(tab_rect, 0, fill),
            egui::Shape::line_segment(
                [tab_rect.left_bottom(), tab_rect.right_bottom()],
                Stroke::new(2.0, accent()),
            ),
        ]);
    }
    const TRAY_RADIUS: f32 = 8.0;
    const TAB_RADIUS: f32 = 6.0;
    const JOIN_RADIUS: f32 = 7.0;

    // `ribbon_tray_layout` 的 2pt 外边距用于容纳投影，不属于承托面的实体范围。
    let tray = tray_outer_rect.shrink(2.0);
    let tab = egui::Rect::from_min_max(
        tab_rect.min - egui::vec2(2.0, 1.0),
        egui::pos2(tab_rect.max.x + 2.0, tray.top() + 1.0),
    );
    let fill = mix_color(surface(), accent_soft(), 0.18);

    // 任意凹多边形不能由 epaint 直接填充，因此用无描边的同色面片铺底，
    // 最后再以一条贝塞尔闭合路径统一描边；视觉上仍是完整的单一曲面。
    let mut shapes = connected_ribbon_fill(tab, tray, Color32::from_black_alpha(5));
    for shape in &mut shapes {
        shape.translate(egui::vec2(0.0, 2.0));
    }
    let mut lower_shadow = connected_ribbon_fill(tab, tray, Color32::from_black_alpha(4));
    for shape in &mut lower_shadow {
        shape.translate(egui::vec2(0.0, 4.0));
    }
    shapes.extend(lower_shadow);
    shapes.extend(connected_ribbon_fill(tab, tray, fill));
    shapes.push(
        egui::epaint::PathShape::closed_line(
            connected_ribbon_outline(tab, tray, TAB_RADIUS, TRAY_RADIUS, JOIN_RADIUS),
            Stroke::new(1.0, border()),
        )
        .into(),
    );
    egui::Shape::Vec(shapes)
}

/// 仅在 MDEX 下层工具条内弱化常态按钮，悬停、按下与主按钮仍保留明确反馈。
pub(crate) fn configure_ribbon_controls(ui: &mut egui::Ui) {
    if !is_mdex() {
        return;
    }
    let stroke = Stroke::new(1.0, border().gamma_multiply(0.3));
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.bg_fill = Color32::TRANSPARENT;
    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.inactive.bg_stroke = stroke;
    widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    widgets.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.noninteractive.bg_stroke = stroke;
}

pub(crate) fn connected_ribbon_fill(
    tab: egui::Rect,
    tray: egui::Rect,
    color: Color32,
) -> Vec<egui::Shape> {
    const TAB_RADIUS: f32 = 6.0;
    const TRAY_RADIUS: f32 = 8.0;
    const JOIN_RADIUS: f32 = 7.0;

    let mut shapes = vec![
        egui::Shape::rect_filled(tray, TRAY_RADIUS, color),
        egui::Shape::rect_filled(tab, TAB_RADIUS, color),
        egui::Shape::rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(tab.left(), tab.top() + TAB_RADIUS),
                egui::pos2(tab.right(), tray.top() + 1.0),
            ),
            0.0,
            color,
        ),
    ];

    let left_start = egui::pos2(tab.left() - JOIN_RADIUS, tray.top());
    let left_end = egui::pos2(tab.left(), tray.top() - JOIN_RADIUS);
    let mut left_join = vec![left_start];
    push_cubic(
        &mut left_join,
        left_start,
        egui::pos2(tab.left() - JOIN_RADIUS * 0.45, tray.top()),
        egui::pos2(tab.left(), tray.top() - JOIN_RADIUS * 0.45),
        left_end,
    );
    left_join.push(egui::pos2(tab.left(), tray.top()));
    shapes.push(egui::Shape::convex_polygon(left_join, color, Stroke::NONE));

    let right_start = egui::pos2(tab.right(), tray.top() - JOIN_RADIUS);
    let right_end = egui::pos2(tab.right() + JOIN_RADIUS, tray.top());
    let mut right_join = vec![right_start];
    push_cubic(
        &mut right_join,
        right_start,
        egui::pos2(tab.right(), tray.top() - JOIN_RADIUS * 0.45),
        egui::pos2(tab.right() + JOIN_RADIUS * 0.45, tray.top()),
        right_end,
    );
    right_join.push(egui::pos2(tab.right(), tray.top()));
    shapes.push(egui::Shape::convex_polygon(right_join, color, Stroke::NONE));
    shapes
}

pub(crate) fn connected_ribbon_outline(
    tab: egui::Rect,
    tray: egui::Rect,
    tab_radius: f32,
    tray_radius: f32,
    join_radius: f32,
) -> Vec<egui::Pos2> {
    let mut points = vec![egui::pos2(tray.left() + tray_radius, tray.top())];
    points.push(egui::pos2(tab.left() - join_radius, tray.top()));
    push_cubic(
        &mut points,
        egui::pos2(tab.left() - join_radius, tray.top()),
        egui::pos2(tab.left() - join_radius * 0.45, tray.top()),
        egui::pos2(tab.left(), tray.top() - join_radius * 0.45),
        egui::pos2(tab.left(), tray.top() - join_radius),
    );
    points.push(egui::pos2(tab.left(), tab.top() + tab_radius));
    push_cubic(
        &mut points,
        egui::pos2(tab.left(), tab.top() + tab_radius),
        egui::pos2(tab.left(), tab.top() + tab_radius * 0.45),
        egui::pos2(tab.left() + tab_radius * 0.45, tab.top()),
        egui::pos2(tab.left() + tab_radius, tab.top()),
    );
    points.push(egui::pos2(tab.right() - tab_radius, tab.top()));
    push_cubic(
        &mut points,
        egui::pos2(tab.right() - tab_radius, tab.top()),
        egui::pos2(tab.right() - tab_radius * 0.45, tab.top()),
        egui::pos2(tab.right(), tab.top() + tab_radius * 0.45),
        egui::pos2(tab.right(), tab.top() + tab_radius),
    );
    points.push(egui::pos2(tab.right(), tray.top() - join_radius));
    push_cubic(
        &mut points,
        egui::pos2(tab.right(), tray.top() - join_radius),
        egui::pos2(tab.right(), tray.top() - join_radius * 0.45),
        egui::pos2(tab.right() + join_radius * 0.45, tray.top()),
        egui::pos2(tab.right() + join_radius, tray.top()),
    );
    points.push(egui::pos2(tray.right() - tray_radius, tray.top()));
    push_cubic(
        &mut points,
        egui::pos2(tray.right() - tray_radius, tray.top()),
        egui::pos2(tray.right() - tray_radius * 0.45, tray.top()),
        egui::pos2(tray.right(), tray.top() + tray_radius * 0.45),
        egui::pos2(tray.right(), tray.top() + tray_radius),
    );
    points.push(egui::pos2(tray.right(), tray.bottom() - tray_radius));
    push_cubic(
        &mut points,
        egui::pos2(tray.right(), tray.bottom() - tray_radius),
        egui::pos2(tray.right(), tray.bottom() - tray_radius * 0.45),
        egui::pos2(tray.right() - tray_radius * 0.45, tray.bottom()),
        egui::pos2(tray.right() - tray_radius, tray.bottom()),
    );
    points.push(egui::pos2(tray.left() + tray_radius, tray.bottom()));
    push_cubic(
        &mut points,
        egui::pos2(tray.left() + tray_radius, tray.bottom()),
        egui::pos2(tray.left() + tray_radius * 0.45, tray.bottom()),
        egui::pos2(tray.left(), tray.bottom() - tray_radius * 0.45),
        egui::pos2(tray.left(), tray.bottom() - tray_radius),
    );
    points.push(egui::pos2(tray.left(), tray.top() + tray_radius));
    push_cubic(
        &mut points,
        egui::pos2(tray.left(), tray.top() + tray_radius),
        egui::pos2(tray.left(), tray.top() + tray_radius * 0.45),
        egui::pos2(tray.left() + tray_radius * 0.45, tray.top()),
        egui::pos2(tray.left() + tray_radius, tray.top()),
    );
    points
}

pub(crate) fn push_cubic(
    points: &mut Vec<egui::Pos2>,
    start: egui::Pos2,
    control_1: egui::Pos2,
    control_2: egui::Pos2,
    end: egui::Pos2,
) {
    const STEPS: usize = 6;
    for step in 1..=STEPS {
        let t = step as f32 / STEPS as f32;
        let one_minus_t = 1.0 - t;
        points.push(egui::pos2(
            one_minus_t.powi(3) * start.x
                + 3.0 * one_minus_t.powi(2) * t * control_1.x
                + 3.0 * one_minus_t * t.powi(2) * control_2.x
                + t.powi(3) * end.x,
            one_minus_t.powi(3) * start.y
                + 3.0 * one_minus_t.powi(2) * t * control_1.y
                + 3.0 * one_minus_t * t.powi(2) * control_2.y
                + t.powi(3) * end.y,
        ));
    }
}

pub(crate) fn mix_color(from: Color32, to: Color32, amount: f32) -> Color32 {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgb(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
    )
}

/// 浮窗入场动画：淡入 + 从 0.96 缩放到 1.0，时长取中档。每帧在窗口渲染后调用，
/// 传入 `Window::show` 返回的 response。
///
/// 缩放中心取窗口自身的中心而不是屏幕中心：可拖动的浮窗（知识库预览、版本对照、
/// 配置版本历史）被挪到边上后，按屏幕中心缩放会斜着弹一下。
/// 动画结束后恢复单位变换，避免每帧重复合成。
pub(crate) fn window_enter_anim(ctx: &egui::Context, id: egui::Id, window: &egui::Response) {
    // 记下这一窗当前的图层，关闭时 `reset_window_anim` 才知道该把谁的变换清掉。
    ctx.data_mut(|data| data.insert_temp(id.with("layer"), window.layer_id));
    let t = ctx.animate_bool_with_time(id, true, anim::MEDIUM);
    let scale = 0.96 + 0.04 * t;
    let center = window.rect.center();
    ctx.set_transform_layer(
        window.layer_id,
        if t >= 1.0 {
            egui::emath::TSTransform::IDENTITY
        } else {
            egui::emath::TSTransform::new(center.to_vec2() * (1.0 - scale), scale)
        },
    );
}

/// 复位浮窗入场动画（窗口关闭时调用），下次打开才能从头重播。
///
/// 顺带把图层变换恢复成单位阵：`Memory::to_global` 不逐帧清空，浮窗如果在动画
/// 中途被关掉，那条带缩放的变换会一直挂在 map 里。
pub(crate) fn reset_window_anim(ctx: &egui::Context, id: egui::Id) {
    ctx.animate_bool_with_time(id, false, 0.0);
    if let Some(layer) = ctx.data(|data| data.get_temp::<egui::LayerId>(id.with("layer"))) {
        ctx.set_transform_layer(layer, egui::emath::TSTransform::IDENTITY);
    }
}
