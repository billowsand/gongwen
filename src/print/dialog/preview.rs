//! 打印预览的几何：页面（点）在纸张（毫米）上的摆放，以及纹理要不要转向。
//!
//! 放在对话框模块里而不是 `raster.rs`：这里算的是界面上的比例矩形（`egui::Rect`），
//! 与 `raster.rs` 的设备像素摆放是两套坐标；而且三个平台都要画预览，`raster.rs`
//! 是 Windows GDI 路径专用的。
//!
//! 「实际大小」按 1 pt = 25.4/72 mm 换算，放得下（容差同 `raster::PAPER_TOLERANCE`）
//! 就在纸上居中，放不下退回「适合纸张」——与 `raster::place_page` 的规则一致。

use eframe::egui;

use crate::print::Scaling;
use crate::print::raster;

/// 1 点等于多少毫米。
const MM_PER_POINT: f32 = 25.4 / 72.0;

/// 常见打印机的四边不可打印边距（毫米）。「适合纸张」按这个留边，与
/// `raster::place_page` 里「缩进可打印区」的意思一致；真实边距逐机不同，
/// 这里只用来画预览，不参与出纸计算。
const PRINTABLE_MARGIN_MM: f32 = 4.2;

/// 预览一页要画的东西。纹理可能还没渲染出来，但页面的点尺寸在打开 PDF 时就已经知道。
pub(crate) struct PreviewPage {
    /// 已经渲染好的纹理；还在队列里就是 `None`。
    pub(crate) texture: Option<egui::TextureId>,
    /// 页面的点尺寸（宽, 高）；打开还没完成时是 `None`。
    pub(crate) page_pt: Option<(f32, f32)>,
}

/// 页面在纸上要不要逆时针转 90°。
///
/// 预览拿不到页面的 `/Rotate` 与介质方向（`PdfSession` 只存算过 `/Rotate` 的显示尺寸），
/// 只能按「显示方向与纸张方向是否一致」判断：竖页落竖纸不转，横页落竖纸转一次。
/// 对 Typst 的横页（竖向介质 + `/Rotate 90`）正好等于 `raster::turns_for_paper`
/// 的结果：撤掉 `/Rotate` 后逆时针转一次。
pub(crate) fn preview_turns(page_pt: (f32, f32), paper_mm: (f32, f32)) -> bool {
    (page_pt.0 > page_pt.1) != (paper_mm.0 > paper_mm.1)
}

/// 页面（点）在纸张（毫米）上的位置，返回相对纸张左上角、以「纸张宽 = 1」归一的矩形。
///
/// 「实际大小」放得下就居中；放不下退回「适合纸张」：等比缩进纸张四边各留
/// [`PRINTABLE_MARGIN_MM`]。横页落在竖纸上时先转向再摆。
pub(crate) fn preview_rect(
    page_pt: (f32, f32),
    paper_mm: (f32, f32),
    scaling: Scaling,
) -> egui::Rect {
    let paper_w = paper_mm.0.max(1.0);
    let paper_h = paper_mm.1.max(1.0);
    let (page_w, page_h) = page_size_on_paper(page_pt, (paper_w, paper_h));
    let tolerance = raster::PAPER_TOLERANCE as f32;
    let fits = page_w <= paper_w * tolerance && page_h <= paper_h * tolerance;
    if scaling == Scaling::Actual && fits {
        return normalized_rect(
            (paper_w - page_w) / 2.0,
            (paper_h - page_h) / 2.0,
            page_w,
            page_h,
            paper_w,
        );
    }
    // 适合纸张：四边各留 PRINTABLE_MARGIN_MM，再等比缩进这个区域并居中。
    let available_w = (paper_w - 2.0 * PRINTABLE_MARGIN_MM).max(1.0);
    let available_h = (paper_h - 2.0 * PRINTABLE_MARGIN_MM).max(1.0);
    let scale = (available_w / page_w).min(available_h / page_h);
    let width = page_w * scale;
    let height = page_h * scale;
    normalized_rect(
        (paper_w - width) / 2.0,
        (paper_h - height) / 2.0,
        width,
        height,
        paper_w,
    )
}

/// 转向之后的页面尺寸（毫米）。
fn page_size_on_paper(page_pt: (f32, f32), paper_mm: (f32, f32)) -> (f32, f32) {
    let width = page_pt.0.max(1.0) * MM_PER_POINT;
    let height = page_pt.1.max(1.0) * MM_PER_POINT;
    if preview_turns(page_pt, paper_mm) {
        (height, width)
    } else {
        (width, height)
    }
}

/// 把一页纹理画进 `rect`；`turned` 为真时逆时针转 90°（与 `raster::rotate_ccw` 同向）。
///
/// `Painter::image` 的 UV 只能翻转，转 90° 得手写一个四边形：把纹理的四个角映射到
/// 目标矩形的四条边。
pub(crate) fn paint_page(
    painter: &egui::Painter,
    texture: egui::TextureId,
    rect: egui::Rect,
    turned: bool,
    tint: egui::Color32,
) {
    let uv = if turned {
        // 逆时针转 90°：源右上角 → 目标左上，源右下角 → 右上，源左下角 → 右下，
        // 源左上角 → 左下。
        [
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
            egui::pos2(0.0, 0.0),
        ]
    } else {
        [
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
        ]
    };
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let mut mesh = egui::Mesh::with_texture(texture);
    for (pos, uv) in corners.into_iter().zip(uv) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv,
            color: tint,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}

/// 毫米坐标换成「纸张宽 = 1」的归一化矩形（横纵用同一个单位，纸的纵横比才对）。
fn normalized_rect(x: f32, y: f32, width: f32, height: f32, paper_w: f32) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(x / paper_w, y / paper_w),
        egui::vec2(width / paper_w, height / paper_w),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4_MM: (f32, f32) = (210.0, 297.0);
    const A4_PT: (f32, f32) = (595.276, 841.89);

    #[test]
    fn a4_page_fills_a4_paper_at_actual_size() {
        let rect = preview_rect(A4_PT, A4_MM, Scaling::Actual);
        assert!(rect.min.x.abs() < 1e-3 && rect.min.y.abs() < 1e-3);
        assert!((rect.width() - 1.0).abs() < 1e-3);
        assert!((rect.height() - 297.0 / 210.0).abs() < 1e-3);
    }

    #[test]
    fn a4_page_on_letter_paper_is_centered_with_margins() {
        // Letter 纸比 A4 矮：实际大小放不下，退回适合纸张，四边至少留 4.2 mm。
        let rect = preview_rect(A4_PT, (215.9, 279.4), Scaling::Actual);
        let margin = PRINTABLE_MARGIN_MM / 215.9;
        let paper_height = 279.4 / 215.9;
        assert!(rect.min.y >= margin - 1e-4);
        assert!(paper_height - rect.max.y >= margin - 1e-4);
        assert!(rect.min.x >= margin - 1e-4);
        assert!(1.0 - rect.max.x >= margin - 1e-4);
    }

    #[test]
    fn a_page_smaller_than_the_paper_keeps_the_difference_on_every_side() {
        // A5 打 A3 纸：实际大小，四边各留差额，居中。
        let rect = preview_rect((419.53, 595.28), (297.0, 420.0), Scaling::Actual);
        assert!((rect.width() - 148.0 / 297.0).abs() < 1e-3);
        assert!((rect.height() - 210.0 / 297.0).abs() < 1e-3);
        assert!((rect.center().x - 0.5).abs() < 1e-3);
        assert!((rect.center().y - 420.0 / 297.0 / 2.0).abs() < 1e-3);
    }

    #[test]
    fn an_a3_page_on_a4_paper_falls_back_to_fitting() {
        let rect = preview_rect((841.89, 1190.55), A4_MM, Scaling::Actual);
        assert!(rect.min.x >= 0.0 && rect.max.x <= 1.0);
        assert!(rect.min.y >= 0.0);
        assert!(rect.height() < 297.0 / 210.0);
    }

    #[test]
    fn fit_leaves_the_printable_margin_on_every_side() {
        let rect = preview_rect(A4_PT, A4_MM, Scaling::Fit);
        let margin = PRINTABLE_MARGIN_MM / 210.0;
        let paper_height = 297.0 / 210.0;
        // 等比缩放：窄的那一边正好留 4.2 mm，另一边只会更多。
        assert!((rect.min.x - margin).abs() < 1e-3);
        assert!((1.0 - rect.max.x - margin).abs() < 1e-3);
        assert!(rect.min.y >= margin - 1e-3);
        assert!(paper_height - rect.max.y >= margin - 1e-3);
        assert!((rect.min.y - (paper_height - rect.max.y)).abs() < 1e-3);
    }

    #[test]
    fn a_landscape_page_is_turned_onto_portrait_paper() {
        // Typst 横页：显示出来是 842 × 595 的横页，转成竖纸方向后正好填满 A4。
        assert!(preview_turns((841.89, 595.28), A4_MM));
        let rect = preview_rect((841.89, 595.28), A4_MM, Scaling::Actual);
        assert!((rect.width() - 1.0).abs() < 1e-3);
        assert!((rect.height() - 297.0 / 210.0).abs() < 1e-3);
        // 竖页落竖纸、横页落横纸都不转。
        assert!(!preview_turns(A4_PT, A4_MM));
        assert!(!preview_turns((841.89, 595.28), (297.0, 210.0)));
    }
}
