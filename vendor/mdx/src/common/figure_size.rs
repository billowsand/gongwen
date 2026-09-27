//! 研究报告插图的默认宽度。
//!
//! 一律铺满版心宽的老办法只管宽不管高：竖图（手机截图、整页扫描件）会高过
//! 版心，小图标会被放大到发糊。这里改按"占多大面积"定宽，再用三条硬上限
//! 兜住，最后量化成四档，全篇的图只有几种宽度，版面整齐。
//!
//! TeX（`tex_research_emitter`）、DOCX（`docx_research`）与 gongwen 的研究
//! 报告预览三处共用 [`width_fraction`]，同一张图在三处一样大。公文不走这里。

use std::fs;
use std::path::Path;

/// 版心宽：`md2tex.cls` 的 `\geometry{width=156mm}`，DOCX 左右边距 28 / 26 mm。
pub const TEXT_WIDTH_MM: f64 = 156.0;
/// 版心高：`\geometry{height=225mm}`，DOCX 上下边距 37 / 35 mm。
pub const TEXT_HEIGHT_MM: f64 = 225.0;

/// 一张图的目标面积占版心面积的比例。宽 w、高 h、宽高比 r 时 w·h = A、
/// w = √(A·r)：宽图自然接近满宽，竖图自然变窄，一个式子管所有比例。
const AREA_RATIO: f64 = 0.38;

/// 图高不超过版心高的这个比例，给图题和上下文留地方。
pub const MAX_HEIGHT_RATIO: f64 = 0.6;

/// 位图打印分辨率的下限：再放大就发糊，宽度不超过 像素宽 ÷ 150 英寸。
const MIN_DPI: f64 = 150.0;

/// 宽度档位（占版心宽的比例），从宽到窄。
pub const WIDTH_STEPS: [f64; 4] = [1.0, 0.8, 0.6, 0.45];

/// 判断宽度是否落在某一档上用的容差，吸收浮点误差。
const EPS: f64 = 1e-6;

/// 一张插图的原始尺寸。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FigureSource {
    /// 位图：像素宽高，参与分辨率上限。
    Raster { width_px: u32, height_px: u32 },
    /// 矢量图（PDF 页面）：只看宽高比，怎么放大都清楚，单位任意。
    Vector { width: f64, height: f64 },
}

impl FigureSource {
    fn size(self) -> (f64, f64) {
        match self {
            Self::Raster {
                width_px,
                height_px,
            } => (f64::from(width_px), f64::from(height_px)),
            Self::Vector { width, height } => (width, height),
        }
    }
}

/// 插图宽度占版心宽的比例，取值在 (0, 1]。
///
/// 1. 按面积算出偏好宽度，取最近的一档（面积是软目标，差一点无妨）；
/// 2. 三条硬上限取最小：不超过版心宽、图高不超过版心高的 60%、位图不低于
///    150 DPI；
/// 3. 偏好档超过硬上限时往下退到上限以内最宽的一档；最窄的一档也放不下
///    （极长的竖图、极小的位图），就照上限原样给，不硬凑档位。
///
/// 尺寸无效（零或非有限值）时返回 1.0，与原先的满宽一致。
pub fn width_fraction(source: FigureSource) -> f64 {
    let (width, height) = source.size();
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return 1.0;
    }
    let ratio = width / height;

    let preferred = (AREA_RATIO * TEXT_WIDTH_MM * TEXT_HEIGHT_MM * ratio).sqrt() / TEXT_WIDTH_MM;
    let mut cap = 1.0_f64.min(MAX_HEIGHT_RATIO * TEXT_HEIGHT_MM * ratio / TEXT_WIDTH_MM);
    if let FigureSource::Raster { width_px, .. } = source {
        let max_mm = f64::from(width_px) / MIN_DPI * 25.4;
        cap = cap.min(max_mm / TEXT_WIDTH_MM);
    }

    let nearest = WIDTH_STEPS
        .iter()
        .copied()
        .min_by(|a, b| (a - preferred).abs().total_cmp(&(b - preferred).abs()))
        .unwrap_or(1.0);
    if nearest <= cap + EPS {
        return nearest;
    }
    WIDTH_STEPS
        .iter()
        .copied()
        .find(|step| *step <= cap + EPS)
        .unwrap_or(cap)
}

/// 读一张本地插图的原始尺寸：PDF 取 `page` 页（从 1 起）的页面尺寸，其余按
/// 位图读文件头。读不出来返回 `None`，调用方退回原先的排法。
pub fn probe(path: &Path, page: Option<u32>) -> Option<FigureSource> {
    let is_pdf = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        let bytes = fs::read(path).ok()?;
        let pdf = hayro_syntax::Pdf::new(bytes).ok()?;
        let index = page.unwrap_or(1).checked_sub(1)? as usize;
        let (width, height) = pdf.pages().get(index)?.render_dimensions();
        return Some(FigureSource::Vector {
            width: f64::from(width),
            height: f64::from(height),
        });
    }
    let (width_px, height_px) = image::image_dimensions(path).ok()?;
    Some(FigureSource::Raster {
        width_px,
        height_px,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 足够大的位图：分辨率上限不起作用，只看面积与限高。
    fn raster(ratio_w: u32, ratio_h: u32) -> FigureSource {
        FigureSource::Raster {
            width_px: ratio_w * 2000,
            height_px: ratio_h * 2000,
        }
    }

    #[test]
    fn common_aspect_ratios_land_on_the_expected_steps() {
        for (source, expected, what) in [
            (raster(16, 9), 1.0, "16:9 宽图铺满"),
            (raster(4, 3), 0.8, "4:3"),
            (raster(1, 1), 0.8, "方图"),
            (raster(3, 4), 0.6, "3:4 竖图"),
            (
                FigureSource::Vector {
                    width: 595.0,
                    height: 842.0,
                },
                0.6,
                "A4 整页 PDF",
            ),
        ] {
            assert_eq!(width_fraction(source), expected, "{what}");
        }
    }

    #[test]
    fn tall_figures_are_held_under_the_height_cap() {
        // 手机竖屏：面积偏好是 0.45 档，但那样图高超出版心 60%，退到限高本身。
        let phone = FigureSource::Raster {
            width_px: 1170,
            height_px: 2532,
        };
        let fraction = width_fraction(phone);
        let height_mm = fraction * TEXT_WIDTH_MM * 2532.0 / 1170.0;
        assert!(fraction < 0.45, "{fraction}");
        assert!(
            height_mm <= MAX_HEIGHT_RATIO * TEXT_HEIGHT_MM + 1e-6,
            "{height_mm}"
        );
        // 每一档的结果都不许超高。
        for (w, h) in [(1, 1), (3, 4), (2, 3), (1, 2), (1, 5)] {
            let fraction = width_fraction(raster(w, h));
            let height_mm = fraction * TEXT_WIDTH_MM * f64::from(h) / f64::from(w);
            assert!(
                height_mm <= MAX_HEIGHT_RATIO * TEXT_HEIGHT_MM + 1e-6,
                "{w}:{h} → {fraction}"
            );
        }
    }

    #[test]
    fn small_bitmaps_are_not_blown_up() {
        // 100 像素宽的小图标：150 DPI 下约 17 mm，远不到最窄一档，照原样给。
        let icon = FigureSource::Raster {
            width_px: 100,
            height_px: 50,
        };
        let fraction = width_fraction(icon);
        assert!((fraction * TEXT_WIDTH_MM - 100.0 / 150.0 * 25.4).abs() < 1e-6);
        // 800 像素宽的 16:9 截图：面积偏好满宽，但满宽只有 130 DPI，退到 0.8 档
        // （163 DPI）。
        let screenshot = FigureSource::Raster {
            width_px: 800,
            height_px: 450,
        };
        assert_eq!(width_fraction(screenshot), 0.8);
    }

    #[test]
    fn vector_figures_ignore_the_resolution_cap() {
        let tiny_but_vector = FigureSource::Vector {
            width: 16.0,
            height: 9.0,
        };
        assert_eq!(width_fraction(tiny_but_vector), 1.0);
    }

    #[test]
    fn degenerate_sizes_fall_back_to_full_width() {
        for source in [
            FigureSource::Raster {
                width_px: 0,
                height_px: 10,
            },
            FigureSource::Vector {
                width: f64::NAN,
                height: 1.0,
            },
        ] {
            assert_eq!(width_fraction(source), 1.0);
        }
    }

    #[test]
    fn probe_reads_bitmap_headers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        image::RgbaImage::new(30, 20).save(&path).unwrap();
        assert_eq!(
            probe(&path, None),
            Some(FigureSource::Raster {
                width_px: 30,
                height_px: 20
            })
        );
        assert_eq!(probe(&dir.path().join("missing.png"), None), None);
    }
}
