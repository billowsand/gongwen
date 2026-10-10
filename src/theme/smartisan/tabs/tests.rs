//! 检查凹角网格不会越界，并在实际设备缩放下出接缝样张。
use super::*;

#[test]
fn paper_join_mesh_stays_finite_and_within_feather() {
    for width in [132.0, 250.0] {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let rect = Rect::from_min_max(egui::pos2(20.0, 12.0), egui::pos2(20.0 + width, 50.0));
            let left = left_edge(rect, 50.0);
            assert!(left.windows(2).all(|pair| pair[0].y <= pair[1].y));
            let mesh = paper_mesh(
                &left,
                rect.center().x,
                Color32::WHITE,
                1.0 / scale,
                Some(50.0),
            );
            assert!(mesh.is_valid());
            for vertex in &mesh.vertices {
                assert!(vertex.pos.is_finite());
                assert!(vertex.pos.x >= rect.left() - JOIN_RADIUS - 1.0 / scale);
                assert!(vertex.pos.x <= rect.right() + JOIN_RADIUS + 1.0 / scale);
                assert!(vertex.pos.y <= 50.0);
            }
            // 底边羽化宽度必须为零，闭合的尖端不能向两侧突出。
            let base_vertices: Vec<_> = mesh
                .vertices
                .iter()
                .filter(|v| (v.pos.y - 50.0).abs() < 0.001)
                .collect();
            assert!(
                base_vertices
                    .iter()
                    .all(|v| (v.pos.x - (rect.left() - JOIN_RADIUS)).abs() < 0.001
                        || (v.pos.x - (rect.right() + JOIN_RADIUS)).abs() < 0.001)
            );
        }
    }
}

#[test]
#[ignore = "出标签接缝与缩放样张，手动跑"]
fn smartisan_tab_join_samples() {
    crate::theme::set_current(crate::models::ThemeName::Smartisan);
    let ctx = egui::Context::default();
    crate::theme::configure_style(&ctx);
    let size = egui::vec2(340.0, 90.0);
    let mut canvas = crate::ui_snapshot::Canvas::default();
    for scale in [1.0, 1.25, 1.5, 2.0, 4.0] {
        ctx.set_pixels_per_point(scale);
        for frame in 0..3 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            let painter = ui.painter();
                            super::super::gradient(
                                painter,
                                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(340.0, 48.0)),
                                Color32::from_rgb(163, 153, 139),
                                Color32::from_rgb(145, 133, 117),
                            );
                            painter.rect_filled(
                                Rect::from_min_max(egui::pos2(0.0, 48.0), egui::pos2(340.0, 90.0)),
                                0,
                                crate::theme::surface(),
                            );
                            document_tab(
                                painter,
                                Rect::from_min_max(egui::pos2(12.0, 16.0), egui::pos2(94.0, 44.0)),
                                false,
                                false,
                                false,
                            );
                            document_tab(
                                painter,
                                Rect::from_min_max(
                                    egui::pos2(110.0, 18.0),
                                    egui::pos2(250.0, 46.0),
                                ),
                                true,
                                false,
                                false,
                            );
                            document_tab(
                                painter,
                                Rect::from_min_max(
                                    egui::pos2(266.0, 16.0),
                                    egui::pos2(328.0, 44.0),
                                ),
                                false,
                                true,
                                false,
                            );
                        });
                },
            );
            if frame < 2 {
                canvas.absorb(&output.textures_delta);
            } else {
                canvas.render(
                    &ctx,
                    output,
                    size,
                    crate::theme::canvas(),
                    std::path::Path::new(&format!(
                        "tmp/smartisan-tab-join-{}.png",
                        (scale * 100.0) as u32
                    )),
                );
            }
        }
    }
    crate::theme::set_current(crate::models::ThemeName::default());
}
