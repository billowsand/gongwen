//! 光栅打印（Windows GDI 路径）用到的纯计算：像素格式转换、按纸张方向转页、
//! 页面在纸上的摆放。与平台无关，放在这里好在任何平台上单测。

#![cfg_attr(not(windows), allow(dead_code))]

/// hayro 给的是预乘 RGBA，GDI 的 32 位 DIB 要 BGRA。底色是白纸，alpha 恒 255。
pub(crate) fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

/// 32 位位图（行优先、自顶向下）逆时针转 `turns` 个 90°（0–3）。返回新位图与新宽高。
pub(crate) fn rotate_ccw(
    pixels: &[u8],
    width: usize,
    height: usize,
    turns: u8,
) -> (Vec<u8>, usize, usize) {
    let turns = turns % 4;
    if turns == 0 {
        return (pixels.to_vec(), width, height);
    }
    let (new_width, new_height) = if turns == 2 {
        (width, height)
    } else {
        (height, width)
    };
    let mut out = vec![0u8; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let (nx, ny) = match turns {
                // 逆时针 90°：原来的右边转到上边。
                1 => (y, width - 1 - x),
                2 => (width - 1 - x, height - 1 - y),
                // 逆时针 270° = 顺时针 90°：原来的左边转到上边。
                _ => (height - 1 - y, x),
            };
            let src = (y * width + x) * 4;
            let dst = (ny * new_width + nx) * 4;
            out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
        }
    }
    (out, new_width, new_height)
}

/// hayro 渲染出的位图要逆时针转几个 90° 才落回纸张方向（0–3）。
///
/// hayro 会按页面的 `/Rotate` 把内容转成「屏幕上该看到的样子」。打印要按介质
/// （MediaBox）方向出：Typst 出的横向附件与整份横排都是「竖向纸张 + 内容逆时针转
/// 90° + `/Rotate 90`」（见 `docs/typst-engine.md`），撤掉 `/Rotate` 才是竖着的纸，
/// 双面打印的装订边也才对得上；否则横的位图塞进竖纸，整页缩到七成左右。
/// 撤掉之后介质方向仍与纸张不符（PDF 本身就是横向介质），再按同一约定逆时针转 90°，
/// 与 CUPS 的自动转向一致。
///
/// `rotate_degrees` 是页面的 `/Rotate`；`media_landscape` 是介质是否横向；
/// `paper_landscape` 是纸张是否横向。
pub(crate) fn turns_for_paper(
    rotate_degrees: u16,
    media_landscape: bool,
    paper_landscape: bool,
) -> u8 {
    // 显示时顺时针转过 R，撤掉就是逆时针转 R。
    let undo = u8::try_from((rotate_degrees % 360) / 90).unwrap_or(0);
    let fit = u8::from(media_landscape != paper_landscape);
    (undo + fit) % 4
}

/// 打印机设备的几何信息，单位都是设备像素（由 `GetDeviceCaps` 读出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceGeometry {
    pub(crate) dpi_x: i32,
    pub(crate) dpi_y: i32,
    /// 整张纸的宽高（`PHYSICALWIDTH` / `PHYSICALHEIGHT`）。
    pub(crate) paper_w: i32,
    pub(crate) paper_h: i32,
    /// 可打印区左上角在纸上的位置（`PHYSICALOFFSETX` / `PHYSICALOFFSETY`）。
    pub(crate) offset_x: i32,
    pub(crate) offset_y: i32,
    /// 可打印区的宽高（`HORZRES` / `VERTRES`）。GDI 的坐标原点就在可打印区左上角。
    pub(crate) area_w: i32,
    pub(crate) area_h: i32,
}

/// 纸张尺寸允许的误差：A4 的 PDF（595.28 × 841.89 pt）与驱动报的 A4 纸差一两个像素
/// 很常见，不能因此判成「放不下」而改成缩印。打印预览的「实际大小」也用它。
pub(crate) const PAPER_TOLERANCE: f64 = 1.02;

/// 页面（点，已转成纸张方向）在设备上的目标矩形 (x, y, w, h)，坐标相对可打印区原点。
///
/// 放得下就按**实际大小**、以整张纸为基准居中：公文的版心、页边距都是按毫米定的，
/// 以前塞进可打印区会按硬件边距整页缩小约 4%，版心量出来不够 156 mm。纸张信息拿
/// 不到或页面比纸大（A3 稿打到 A4 上），退回按可打印区等比缩放。`force_fit` 为真
/// 时跳过实际大小，直接走「适合纸张」。
pub(crate) fn place_page(
    page_w_pt: f32,
    page_h_pt: f32,
    device: &DeviceGeometry,
    force_fit: bool,
) -> (i32, i32, i32, i32) {
    let page_w = f64::from(page_w_pt.max(1.0)) / 72.0 * f64::from(device.dpi_x);
    let page_h = f64::from(page_h_pt.max(1.0)) / 72.0 * f64::from(device.dpi_y);
    // 用户选了「适合纸张」：无论放不放得下都缩进可打印区，给非 A4 来源的 PDF 用。
    if force_fit {
        return fit_rect(page_w, page_h, device.area_w, device.area_h);
    }
    let paper_known =
        device.paper_w > 0 && device.paper_h > 0 && device.dpi_x > 0 && device.dpi_y > 0;
    if paper_known
        && page_w <= f64::from(device.paper_w) * PAPER_TOLERANCE
        && page_h <= f64::from(device.paper_h) * PAPER_TOLERANCE
    {
        let x = (f64::from(device.paper_w) - page_w) / 2.0 - f64::from(device.offset_x);
        let y = (f64::from(device.paper_h) - page_h) / 2.0 - f64::from(device.offset_y);
        return (to_i32(x), to_i32(y), to_extent(page_w), to_extent(page_h));
    }
    fit_rect(page_w, page_h, device.area_w, device.area_h)
}

/// 把页面按纵横比放进可打印区域并居中。
fn fit_rect(src_w: f64, src_h: f64, area_w: i32, area_h: i32) -> (i32, i32, i32, i32) {
    let scale = f64::from(area_w) / src_w.max(1.0);
    let scale = scale.min(f64::from(area_h) / src_h.max(1.0));
    let w = to_extent(src_w * scale);
    let h = to_extent(src_h * scale);
    ((area_w - w) / 2, (area_h - h) / 2, w, h)
}

/// Rust 的浮点转整数本身是饱和的（NaN 得 0），不会出未定义行为；这里显式夹住，
/// 尺寸至少一个像素。
fn to_extent(value: f64) -> i32 {
    if value.is_nan() {
        1
    } else {
        value.round().clamp(1.0, f64::from(i32::MAX)) as i32
    }
}

fn to_i32(value: f64) -> i32 {
    if value.is_nan() {
        0
    } else {
        value
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 600 DPI 的 A4 激光打印机：纸 4960 × 7016，四边各约 4.2 mm（100 像素）不可打印。
    fn a4_600dpi() -> DeviceGeometry {
        DeviceGeometry {
            dpi_x: 600,
            dpi_y: 600,
            paper_w: 4960,
            paper_h: 7016,
            offset_x: 100,
            offset_y: 100,
            area_w: 4760,
            area_h: 6816,
        }
    }

    #[test]
    fn a4_page_prints_at_actual_size_aligned_to_the_paper() {
        let device = a4_600dpi();
        let (x, y, w, h) = place_page(595.28, 841.89, &device, false);
        // 实际大小：宽 210 mm = 4961 像素左右，不按可打印区缩小。
        assert!((w - 4961).abs() <= 1, "宽 {w}");
        assert!((h - 7016).abs() <= 1, "高 {h}");
        // 原点按纸张算：可打印区原点在纸上 (100, 100)，所以页面左上角落在 (-100, -100) 附近。
        assert!((x + 100).abs() <= 1 && (y + 100).abs() <= 1, "({x}, {y})");
    }

    #[test]
    fn oversized_page_falls_back_to_fitting_the_printable_area() {
        let device = a4_600dpi();
        // A3 稿打到 A4 纸上：放不下，等比缩进可打印区。
        let (x, y, w, h) = place_page(841.89, 1190.55, &device, false);
        assert!(w <= device.area_w && h <= device.area_h);
        assert!(x >= 0 && y >= 0);
    }

    #[test]
    fn unknown_paper_size_falls_back_to_fitting() {
        let mut device = a4_600dpi();
        device.paper_w = 0;
        let (_, _, w, h) = place_page(595.28, 841.89, &device, false);
        assert!(w <= device.area_w && h <= device.area_h);
    }

    #[test]
    fn fit_mode_shrinks_an_a4_page_into_the_printable_area() {
        let device = a4_600dpi();
        // A4 稿放得下，但用户选了「适合纸张」：照样缩进可打印区并居中。
        let (x, y, w, h) = place_page(595.28, 841.89, &device, true);
        assert_eq!(w, device.area_w);
        assert!(h <= device.area_h);
        assert_eq!(x, 0);
        assert!(y > 0);
    }

    #[test]
    fn fit_never_collapses_to_zero() {
        let (_, _, w, h) = fit_rect(1.0, 1.0, 1, 1);
        assert!(w >= 1 && h >= 1);
    }

    #[test]
    fn rgba_to_bgra_swaps_red_and_blue_and_forces_opaque() {
        let bgra = rgba_to_bgra(&[10, 20, 30, 40, 200, 100, 50, 0]);
        assert_eq!(bgra, vec![30, 20, 10, 255, 50, 100, 200, 255]);
    }

    /// 2 × 1 的位图：左像素 A、右像素 B。
    fn two_by_one() -> Vec<u8> {
        vec![1, 1, 1, 1, 2, 2, 2, 2]
    }

    #[test]
    fn rotate_counter_clockwise_puts_right_pixel_on_top() {
        let (out, w, h) = rotate_ccw(&two_by_one(), 2, 1, 1);
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![2, 2, 2, 2, 1, 1, 1, 1]);
    }

    #[test]
    fn three_quarter_turns_put_left_pixel_on_top() {
        let (out, w, h) = rotate_ccw(&two_by_one(), 2, 1, 3);
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![1, 1, 1, 1, 2, 2, 2, 2]);
    }

    #[test]
    fn half_turn_reverses_pixels() {
        let (out, w, h) = rotate_ccw(&two_by_one(), 2, 1, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, vec![2, 2, 2, 2, 1, 1, 1, 1]);
    }

    #[test]
    fn four_quarter_turns_restore_the_original() {
        let pixels: Vec<u8> = (0..(3 * 2 * 4)).map(|value| value as u8).collect();
        let (once, w, h) = rotate_ccw(&pixels, 3, 2, 1);
        let (back, w, h) = rotate_ccw(&once, w, h, 3);
        assert_eq!((w, h), (3, 2));
        assert_eq!(back, pixels);
    }

    #[test]
    fn landscape_attachment_is_turned_back_to_portrait_paper() {
        // Typst 的横向附件：竖向介质 + /Rotate 90。撤掉 /Rotate 就是竖的。
        assert_eq!(turns_for_paper(90, false, false), 1);
        assert_eq!(turns_for_paper(270, false, false), 3);
        // 普通竖页不转。
        assert_eq!(turns_for_paper(0, false, false), 0);
        // 横向介质、没有 /Rotate：逆时针转进竖纸。
        assert_eq!(turns_for_paper(0, true, false), 1);
        // 横向纸张上打横向介质：不转。
        assert_eq!(turns_for_paper(0, true, true), 0);
        // 横向介质又带 /Rotate 90：撤掉再转一次，共 180°。
        assert_eq!(turns_for_paper(90, true, false), 2);
    }
}
