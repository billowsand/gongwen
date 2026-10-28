//! 对照预览的推稿定位标：常驻行灯带与箭头，加粗光标缓慢闪烁。
//! 闪烁状态独立于当帧的源码映射，避免绘制结束清映射时把计时也清掉。

use eframe::egui::{self, Color32, Pos2, Stroke};
use std::sync::Arc;

pub(super) const INK: Color32 = Color32::from_rgb(20, 83, 160);
const TINT: Color32 = Color32::from_rgba_premultiplied(8, 18, 30, 32);
const ON: f64 = 0.75;
const OFF: f64 = 0.45;

#[derive(Clone)]
struct Blink {
    source: Arc<str>,
    byte: usize,
    frame: u64,
    focused: bool,
    since: f64,
}

fn blink_id() -> egui::Id {
    egui::Id::new("gw-preview-cursor-blink")
}

/// 同一光标跨帧保留相位；输入、移动、恢复窗口焦点或重新进入预览都先亮起。
pub(super) fn track(ctx: &egui::Context, source: Arc<str>, byte: usize) {
    let now = ctx.input(|input| input.time);
    let focused = ctx.input(|input| input.focused);
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| {
        let since = data
            .get_temp::<Blink>(blink_id())
            .filter(|old| {
                old.byte == byte
                    && old.source == source
                    && old.frame.saturating_add(1) >= frame
                    && old.focused == focused
            })
            .map_or(now, |old| old.since);
        data.insert_temp(
            blink_id(),
            Blink {
                source,
                byte,
                frame,
                focused,
                since,
            },
        );
    });
}

fn phase(elapsed: f64) -> (bool, f32) {
    let phase = elapsed.max(0.0) % (ON + OFF);
    if phase < ON {
        (true, (ON - phase) as f32)
    } else {
        (false, (ON + OFF - phase) as f32)
    }
}

/// 灯带在文字之前铺，保持字形与花脸稿标记清晰；不扩大命中或滚动区域。
pub(super) fn paint_backdrop(ui: &egui::Ui, origin: Pos2, top: f32, height: f32, width: f32) {
    if !ui.input(|input| input.focused) {
        return;
    }
    ui.painter().rect_filled(
        egui::Rect::from_min_size(origin + egui::vec2(0.0, top), egui::vec2(width, height))
            .expand2(egui::vec2(3.0, 1.0)),
        3.0,
        TINT,
    );
}

pub(super) fn paint(
    ui: &egui::Ui,
    row: &egui::Galley,
    index: usize,
    origin: Pos2,
    tint_top: f32,
    height: f32,
) {
    if !ui.input(|input| input.focused) {
        return;
    }
    let top = origin.y + tint_top;
    let bottom = top + height;
    let center = (top + bottom) / 2.0;
    // 箭头在行左侧留白常驻，光标熄灭时仍能一眼找到当前行。
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(origin.x - 13.0, center - 5.0),
            egui::pos2(origin.x - 6.0, center),
            egui::pos2(origin.x - 13.0, center + 5.0),
        ],
        INK,
        Stroke::new(1.0, Color32::WHITE),
    ));
    let since = ui
        .ctx()
        .data(|data| data.get_temp::<Blink>(blink_id()))
        .map(|blink| blink.since);
    let Some(since) = since else { return };
    let (visible, wait) = phase(ui.input(|input| input.time) - since);
    if ui.visuals().text_cursor.blink {
        // 只在切换明暗时唤醒，不以动画帧率持续排版长稿。
        if ui.is_rect_visible(egui::Rect::from_min_max(
            egui::pos2(origin.x - 14.0, top),
            egui::pos2(origin.x + row.size().x, bottom),
        )) {
            ui.ctx().request_repaint_after_secs(wait);
        }
        if !visible {
            return;
        }
    }
    let x = origin.x + row.pos_from_cursor(egui::text::CCursor::new(index)).left();
    let lines = [
        [egui::pos2(x, top + 1.0), egui::pos2(x, bottom - 1.0)],
        [
            egui::pos2(x - 3.0, top + 1.0),
            egui::pos2(x + 3.0, top + 1.0),
        ],
        [
            egui::pos2(x - 3.0, bottom - 1.0),
            egui::pos2(x + 3.0, bottom - 1.0),
        ],
    ];
    for stroke in [Stroke::new(5.0, Color32::WHITE), Stroke::new(3.0, INK)] {
        for line in lines {
            ui.painter().line_segment(line, stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "生成推稿定位标亮暗两相样张，供目视检查"]
    fn preview_cursor_samples() {
        let ctx = egui::Context::default();
        crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let size = egui::vec2(840.0, 360.0);
        let source = "各单位要认真落实会议部署，统筹推进重点任务，及时梳理工作进展。加强协同配合，明确责任分工，逐项推动工作落地见效。对推进过程中发现的问题，要及时研究解决，确保各项任务按期完成。";
        let byte = source.find("明确责任").unwrap();
        let mut canvas = crate::ui_snapshot::Canvas::default();
        for (time, name) in [(0.0, "on"), (0.8, "off")] {
            let output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    focused: true,
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    ui.painter().rect_filled(ui.max_rect(), 0.0, Color32::WHITE);
                    ui.add_space(55.0);
                    ui.horizontal(|ui| {
                        ui.add_space(65.0);
                        ui.vertical(|ui| {
                            let mut metrics = crate::preview::Metrics::research(1000.0, Some(1.0));
                            metrics.content = 680.0;
                            crate::preview::set_text_cursor(&ctx, source, Some(byte));
                            let segments = [crate::preview::ClickableSourceSegment {
                                source: 0..source.len(),
                                chars: 0..source.chars().count(),
                            }];
                            crate::preview::clickable_body_block(
                                ui,
                                &metrics,
                                source,
                                &[],
                                false,
                                &segments,
                                Some(&(0..source.len())),
                                &mut false,
                                &mut None,
                            );
                            crate::preview::set_text_cursor(&ctx, source, None);
                        });
                    });
                },
            );
            canvas.render(
                &ctx,
                output,
                size,
                Color32::WHITE,
                &std::path::PathBuf::from(format!("tmp/preview-samples/cursor-{name}.png")),
            );
        }
    }

    /// 走真实的逐帧登记、绘制与帧尾清理，防止每帧重新计时导致永远不灭。
    #[test]
    fn preview_cursor_blinks_and_restarts_after_edit_or_move() {
        let ctx = egui::Context::default();
        let frame = |time, source: &str, byte, focused, blink| {
            let output = crate::test_support::run_ui(
                &ctx,
                egui::RawInput {
                    time: Some(time),
                    focused,
                    ..Default::default()
                },
                |ui| {
                    ui.visuals_mut().text_cursor.blink = blink;
                    crate::preview::set_text_cursor(&ctx, source, Some(byte));
                    let row = ui.painter().layout_no_wrap(
                        "abcdef".into(),
                        egui::FontId::proportional(24.0),
                        Color32::BLACK,
                    );
                    let origin = egui::pos2(80.0, 80.0);
                    paint_backdrop(ui, origin, 0.0, 32.0, 300.0);
                    paint(ui, &row, byte, origin, 0.0, 32.0);
                    crate::preview::set_text_cursor(&ctx, source, None);
                },
            );
            let bars = output.shapes.iter().filter(|shape| matches!(
                shape.shape, egui::Shape::LineSegment { stroke, .. } if stroke == Stroke::new(3.0, INK)
            )).count();
            let arrows = output
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(
                        &shape.shape, egui::Shape::Path(path) if path.fill == INK
                    )
                })
                .count();
            let bands = output
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(
                        &shape.shape, egui::Shape::Rect(rect) if rect.fill == TINT
                    )
                })
                .count();
            (bars, arrows, bands)
        };
        assert_eq!(frame(0.0, "abcdef", 1, true, true), (3, 1, 1));
        assert_eq!(
            frame(0.8, "abcdef", 1, true, true),
            (0, 1, 1),
            "暗相位仍能找行"
        );
        assert_eq!(frame(1.21, "abcdef", 1, true, true), (3, 1, 1));
        assert_eq!(frame(2.01, "abcdef", 1, true, true), (0, 1, 1));
        assert_eq!(
            frame(2.02, "abcdef", 2, true, true),
            (3, 1, 1),
            "移动立刻亮起"
        );
        assert_eq!(frame(2.82, "abcdef", 2, true, true), (0, 1, 1));
        assert_eq!(
            frame(2.83, "abXdef", 2, true, true),
            (3, 1, 1),
            "同长度替换也重置相位"
        );
        assert_eq!(
            frame(3.63, "abXdef", 2, true, false),
            (3, 1, 1),
            "遵守关闭闪烁的偏好"
        );
        assert_eq!(frame(3.64, "abXdef", 2, false, true), (0, 0, 0));
        assert_eq!(
            frame(3.65, "abXdef", 2, true, true),
            (3, 1, 1),
            "恢复窗口焦点先亮起"
        );
        // 中间有一帧没画预览（例如切到源码模式），回来也从亮相位开始。
        crate::test_support::run_ui(
            &ctx,
            egui::RawInput {
                time: Some(4.0),
                ..Default::default()
            },
            |_| {},
        );
        assert_eq!(frame(4.45, "abXdef", 2, true, true), (3, 1, 1));
    }
}
