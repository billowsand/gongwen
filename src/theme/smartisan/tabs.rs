//! 标签纸页的单一轮廓：单调带状网格填充凹角，填充与渐隐描边共用采样点。
use super::{Color32, Rect, Stroke, egui};

const TOP_RADIUS: f32 = 7.0;
const JOIN_RADIUS: f32 = 8.0;
const ARC_STEPS: usize = 20;

/// 左边界从顶端到纸面，右边界镜像；四分之一圆在连接点与直边保持相切。
fn left_edge(tab: Rect, base: f32) -> Vec<egui::Pos2> {
    let top_radius = TOP_RADIUS.min(tab.width() / 4.0);
    let join_radius = JOIN_RADIUS.min((base - tab.top()) / 3.0);
    let mut points = Vec::with_capacity(ARC_STEPS * 2 + 3);
    for i in 0..=ARC_STEPS {
        let angle = i as f32 / ARC_STEPS as f32 * std::f32::consts::FRAC_PI_2;
        points.push(egui::pos2(
            tab.left() + top_radius * (1.0 - angle.sin()),
            tab.top() + top_radius * (1.0 - angle.cos()),
        ));
    }
    points.push(egui::pos2(tab.left(), base - join_radius));
    for i in 1..=ARC_STEPS {
        let angle = i as f32 / ARC_STEPS as f32 * std::f32::consts::FRAC_PI_2;
        points.push(egui::pos2(
            tab.left() - join_radius * (1.0 - angle.cos()),
            base - join_radius * (1.0 - angle.sin()),
        ));
    }
    points
}

fn vertex(mesh: &mut egui::Mesh, pos: egui::Pos2, color: Color32) {
    mesh.vertices.push(egui::epaint::Vertex {
        pos,
        uv: egui::epaint::WHITE_UV,
        color,
    });
}

fn quad(mesh: &mut egui::Mesh, a: u32, b: u32, c: u32, d: u32) {
    mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
}

/// 凹轮廓不能交给 convex_polygon；逐带三角化不会越过曲线生成细小尖角。
fn paper_mesh(
    left: &[egui::Pos2],
    axis: f32,
    fill: Color32,
    feather: f32,
    fade_base: Option<f32>,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    for p in left {
        vertex(&mut mesh, *p, fill);
        vertex(&mut mesh, egui::pos2(2.0 * axis - p.x, p.y), fill);
    }
    for i in 0..left.len() - 1 {
        let a = i as u32 * 2;
        quad(&mut mesh, a, a + 1, a + 3, a + 2);
    }
    let contour: Vec<_> = left
        .iter()
        .copied()
        .chain(left.iter().rev().map(|p| egui::pos2(2.0 * axis - p.x, p.y)))
        .collect();
    let start = mesh.vertices.len() as u32;
    for (i, p) in contour.iter().enumerate() {
        let previous = contour[(i + contour.len() - 1) % contour.len()];
        let next = contour[(i + 1) % contour.len()];
        let before = (*p - previous).normalized();
        let after = (next - *p).normalized();
        let before_normal = egui::vec2(-before.y, before.x);
        let after_normal = egui::vec2(-after.y, after.x);
        let normal = (before_normal + after_normal).normalized();
        // 脚边收掉羽化外扩，避免闭合底边的法线在零厚度尖端生成小凸点。
        let fade = fade_base.map_or(1.0, |base| ((base - p.y) / JOIN_RADIUS).clamp(0.0, 1.0));
        let length = feather * fade / normal.dot(after_normal).max(0.5);
        vertex(&mut mesh, *p, fill);
        vertex(&mut mesh, *p + normal * length, Color32::TRANSPARENT);
    }
    for i in 0..contour.len() {
        let a = start + i as u32 * 2;
        let b = start + ((i + 1) % contour.len()) as u32 * 2;
        quad(&mut mesh, a, b, b + 1, a + 1);
    }
    mesh
}

/// 描边沿同一轮廓形成一条带，末端透明收口，避免独立线段的端帽和叠色。
fn outline_mesh(points: &[egui::Pos2], base: f32, feather: f32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    for (i, p) in points.iter().enumerate() {
        let previous = points[i.saturating_sub(1)];
        let next = points[(i + 1).min(points.len() - 1)];
        let direction = (next - previous).normalized();
        let normal = egui::vec2(-direction.y, direction.x);
        let fade = ((base - p.y) / JOIN_RADIUS).clamp(0.0, 1.0);
        let color = Color32::from_rgba_unmultiplied(111, 97, 80, (70.0 * fade) as u8);
        for (offset, ink) in [
            (-0.35 - feather, Color32::TRANSPARENT),
            (-0.35, color),
            (0.35, color),
            (0.35 + feather, Color32::TRANSPARENT),
        ] {
            vertex(&mut mesh, *p + normal * offset, ink);
        }
    }
    for i in 0..points.len() - 1 {
        for lane in 0..3 {
            let a = i as u32 * 4 + lane;
            quad(&mut mesh, a, a + 4, a + 5, a + 1);
        }
    }
    mesh
}

fn attached_tab(tab: Rect, base: f32, feather: f32) -> Vec<egui::Shape> {
    let left = left_edge(tab, base);
    let axis = tab.center().x;
    let outline: Vec<_> = left
        .iter()
        .rev()
        .copied()
        .chain(left.iter().map(|p| egui::pos2(2.0 * axis - p.x, p.y)))
        .collect();
    vec![
        egui::Shape::mesh(paper_mesh(
            &left,
            axis,
            crate::theme::surface(),
            feather,
            Some(base),
        )),
        egui::Shape::mesh(outline_mesh(&outline, base, feather)),
    ]
}

#[cfg(test)]
pub fn document_tab(
    painter: &egui::Painter,
    rect: Rect,
    selected: bool,
    hovered: bool,
    pressed: bool,
) {
    document_tab_to(
        painter,
        rect,
        selected,
        hovered,
        pressed,
        rect.bottom() + 2.0,
    );
}

pub fn document_tab_to(
    painter: &egui::Painter,
    rect: Rect,
    selected: bool,
    hovered: bool,
    pressed: bool,
    base: f32,
) {
    if selected {
        // 与标签栏底部的整幅纸面共用基线，不描底边或凹角边线。
        let tab = Rect::from_min_max(
            rect.min - egui::vec2(0.0, 2.0),
            egui::pos2(rect.right(), base),
        );
        // 只放开上下纸边，不突破标签滚动区的左右裁剪。
        let painter = painter.with_clip_rect(painter.clip_rect().expand2(egui::vec2(0.0, 8.0)));
        painter.add(egui::Shape::mesh(paper_mesh(
            &left_edge(tab, base),
            tab.center().x,
            crate::theme::surface(),
            1.0 / painter.ctx().pixels_per_point(),
            Some(base),
        )));
    } else {
        // 常态直接露出栏底材质，只在操作时给一层轻薄反馈。
        if hovered || pressed {
            painter.rect_filled(
                rect,
                6,
                if pressed {
                    Color32::from_black_alpha(16)
                } else {
                    Color32::from_white_alpha(18)
                },
            );
        }
    }
}

/// 功能分区同样使用纸页轮廓，托盘分隔线在脚边留出羽化宽度。
pub fn ribbon_shape(tab: Rect, tray: Rect) -> egui::Shape {
    let tray = tray.expand2(egui::vec2(10.0, 0.0));
    let base = tray.top();
    let tab = Rect::from_min_max(
        tab.min - egui::vec2(2.0, 1.0),
        egui::pos2(tab.right() + 2.0, base),
    );
    let mut shapes = vec![super::gradient_shape(
        tray,
        crate::theme::surface(),
        Color32::from_rgb(248, 242, 230),
    )];
    shapes.extend(attached_tab(tab, base, 0.7));
    let stroke = Stroke::new(0.7, crate::theme::border());
    shapes.push(egui::Shape::line_segment(
        [
            tray.left_top(),
            egui::pos2(tab.left() - JOIN_RADIUS - 1.0, base),
        ],
        stroke,
    ));
    shapes.push(egui::Shape::line_segment(
        [
            egui::pos2(tab.right() + JOIN_RADIUS + 1.0, base),
            tray.right_top(),
        ],
        stroke,
    ));
    shapes.push(egui::Shape::line_segment(
        [tray.left_bottom(), tray.right_bottom()],
        stroke,
    ));
    shapes.push(egui::Shape::line_segment(
        [
            tray.left_bottom() + egui::vec2(0.0, 1.0),
            tray.right_bottom() + egui::vec2(0.0, 1.0),
        ],
        Stroke::new(1.0, Color32::from_black_alpha(12)),
    ));
    egui::Shape::Vec(shapes)
}

/// 圆角控件内的渐变共用抗锯齿边缘，避免内嵌方形渐变露出四个亮角。
pub fn rounded_gradient(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    top: Color32,
    bottom: Color32,
) {
    let radius = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let mut left = Vec::with_capacity(ARC_STEPS * 2 + 2);
    for i in 0..=ARC_STEPS {
        let angle = i as f32 / ARC_STEPS as f32 * std::f32::consts::FRAC_PI_2;
        left.push(egui::pos2(
            rect.left() + radius * (1.0 - angle.sin()),
            rect.top() + radius * (1.0 - angle.cos()),
        ));
    }
    left.push(egui::pos2(rect.left(), rect.bottom() - radius));
    for i in 1..=ARC_STEPS {
        let angle = i as f32 / ARC_STEPS as f32 * std::f32::consts::FRAC_PI_2;
        left.push(egui::pos2(
            rect.left() + radius * (1.0 - angle.cos()),
            rect.bottom() - radius * (1.0 - angle.sin()),
        ));
    }
    let mut mesh = paper_mesh(
        &left,
        rect.center().x,
        Color32::WHITE,
        1.0 / painter.ctx().pixels_per_point(),
        None,
    );
    for v in &mut mesh.vertices {
        if v.color != Color32::TRANSPARENT {
            v.color = top.lerp_to_gamma(
                bottom,
                ((v.pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
            );
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests;
