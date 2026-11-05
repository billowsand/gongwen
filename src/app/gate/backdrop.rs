//! 用不含任何用户数据的抽象三栏生成磨砂底图，只在主题变化时重建。
use crate::theme;
use eframe::egui;

pub(super) fn paint(painter: &egui::Painter, rect: egui::Rect, opacity: f32) {
    let colors = [
        theme::canvas(),
        theme::surface(),
        theme::surface_sunk(),
        theme::border(),
    ];
    let id = egui::Id::new(("gate_frost", colors));
    let cached = painter
        .ctx()
        .data(|data| data.get_temp::<egui::TextureHandle>(id));
    let texture = cached.unwrap_or_else(|| {
        let mut raster = image::RgbaImage::from_pixel(160, 100, image::Rgba(colors[0].to_array()));
        for (x, y, w, h, color) in [
            (5, 8, 31, 85, colors[2]),
            (42, 8, 74, 85, colors[1]),
            (123, 8, 32, 85, colors[2]),
        ] {
            for row in y..y + h {
                for column in x..x + w {
                    raster.put_pixel(column, row, image::Rgba(color.to_array()));
                }
            }
        }
        // 只表达排版节奏，没有字符、稿件标题或真实截图。
        let ink = colors[0].lerp_to_gamma(colors[3], 0.16);
        for (x, y, w) in [
            (10, 20, 19),
            (10, 31, 14),
            (54, 25, 43),
            (52, 39, 53),
            (52, 49, 45),
            (52, 59, 50),
            (52, 69, 41),
            (130, 22, 17),
            (130, 37, 12),
        ] {
            for row in y..y + 2 {
                for column in x..x + w {
                    raster.put_pixel(column, row, image::Rgba(ink.to_array()));
                }
            }
        }
        let raster = image::imageops::blur(&raster, 4.0);
        let image = egui::ColorImage::from_rgba_unmultiplied([160, 100], raster.as_raw());
        let texture = painter
            .ctx()
            .load_texture("lock-frost", image, egui::TextureOptions::LINEAR);
        painter
            .ctx()
            .data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    });
    let mut painter = painter.clone();
    painter.multiply_opacity(opacity.clamp(0.0, 1.0));
    painter.image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    // 统一雾面，确保浅色、深色主题都保持安静的对比度。
    painter.rect_filled(rect, 0.0, colors[0].gamma_multiply(0.62));
}
