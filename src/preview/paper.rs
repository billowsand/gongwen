//! 锤子便签式预览纸面：装饰留在页边，不参与正文排版与导出。

use super::Metrics;
use crate::theme;
use eframe::egui::{self, Color32, Rect, Shape, Stroke, StrokeKind};

fn note_active(metrics: &Metrics) -> bool {
    theme::paper::smartisan_note() && !metrics.research_fonts
}

/// 连续纸面沿用原有框架尺寸，装饰不增加内边距。
pub(super) fn frame(metrics: &Metrics) -> egui::Frame {
    let frame = egui::Frame::new()
        .fill(theme::paper::bg())
        .stroke(Stroke::new(1.0, theme::paper::border()))
        .corner_radius(egui::CornerRadius::same(3))
        .shadow(theme::paper_shadow(theme::paper::shadow_alpha()));
    if note_active(metrics) {
        frame
            .stroke(Stroke::new(1.0, Color32::from_rgb(226, 218, 205)))
            .shadow(note_shadow(metrics))
    } else {
        frame
    }
}

fn note_shadow(metrics: &Metrics) -> egui::epaint::Shadow {
    let scale = metrics.scale;
    egui::epaint::Shadow {
        offset: [0, (3.0 * scale).round().clamp(1.0, 12.0) as i8],
        blur: (10.0 * scale).round().clamp(4.0, 40.0) as u8,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(88, 66, 43, 36),
    }
}

/// 替换预留的背景槽，始终画在文字、选区和表格之下。
pub(super) fn decorate(ui: &egui::Ui, metrics: &Metrics, rect: Rect, slot: egui::layers::ShapeIdx) {
    if note_active(metrics) {
        ui.painter()
            .set(slot, Shape::Vec(note_shapes(metrics, rect)));
    }
}

/// 真分页的呈批件与电话记录单共用同一套纸底和装饰。
pub(super) fn shape(metrics: &Metrics, rect: Rect, border: Color32) -> Shape {
    if note_active(metrics) {
        Shape::Vec(vec![
            frame(metrics).paint(rect),
            Shape::Vec(note_shapes(metrics, rect)),
        ])
    } else {
        egui::epaint::RectShape::new(
            rect,
            3.0,
            theme::paper::bg(),
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        )
        .into()
    }
}

fn note_shapes(metrics: &Metrics, rect: Rect) -> Vec<Shape> {
    let scale = metrics.scale;
    let inset = metrics.pt(18.0);
    let gap = metrics.pt(2.5);
    let outer = rect.shrink(inset);
    let inner = outer.shrink(gap);
    let line = Stroke::new((0.65 * scale).max(0.5), Color32::from_rgb(234, 229, 217));
    let mut shapes = Vec::with_capacity(8);

    // 顶部一小段轻压暗，渐隐到暖白纸底；不铺纹理，不影响文字对比度。
    let top = rect.shrink(1.0);
    let bottom = (top.top() + metrics.pt(26.0)).min(top.bottom());
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (top.left_top(), Color32::from_black_alpha(8)),
        (top.right_top(), Color32::from_black_alpha(8)),
        (egui::pos2(top.right(), bottom), Color32::TRANSPARENT),
        (egui::pos2(top.left(), bottom), Color32::TRANSPARENT),
    ] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    shapes.push(Shape::mesh(mesh));
    if inner.is_positive() {
        shapes.push(Shape::rect_stroke(outer, 0.0, line, StrokeKind::Inside));
        shapes.push(Shape::rect_stroke(
            inner,
            0.0,
            Stroke::new(line.width, Color32::from_rgb(239, 234, 223)),
            StrokeKind::Inside,
        ));
        // 外框四角用小方形收口，尺寸跟随纸张缩放。
        for corner in [
            outer.left_top(),
            outer.right_top(),
            outer.left_bottom(),
            outer.right_bottom(),
        ] {
            shapes.push(Shape::rect_stroke(
                Rect::from_center_size(corner, egui::Vec2::splat(metrics.pt(3.0))),
                0.0,
                line,
                StrokeKind::Inside,
            ));
        }
    }
    shapes
}
