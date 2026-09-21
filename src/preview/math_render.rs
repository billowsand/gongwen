//! 研究报告公式的预览渲染：latex-rust 进程内排版 + PNG 光栅化。
//!
//! 不走 tectonic 往返——整文件编译是秒级延迟，跟不上逐帧预览；这里解析加光栅化
//! 都是微秒级。导出端仍以 tectonic + amsmath 为准，所以预览字形（STIX Two Math）
//! 与导出 PDF（CM 数学字体）不一致属预期，排版尺寸以导出为准。
//!
//! 已知的预览限制：`\text{中文}` 这类中文混排会因 STIX Two Math 缺字形而失败，
//! 预览侧画占位框（导出 PDF 不受影响）。

use eframe::egui;
use latex_rust::{Color as MathColor, Dim, MathFont, MathStyle, PngBackground, PngOptions};
use std::sync::OnceLock;

/// 排版或光栅化失败。预览把错误画成占位框，不向上抛 panic。
#[derive(Debug, Clone)]
pub(crate) struct MathError(pub(crate) String);

impl std::fmt::Display for MathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<latex_rust::Error> for MathError {
    fn from(error: latex_rust::Error) -> Self {
        Self(error.to_string())
    }
}

/// 一份排好版、光栅化完的公式。
pub(crate) struct RenderedMath {
    /// 物理像素图（按 OVERSAMPLE 超采样，显示时按 `size` 缩小回逻辑尺寸）。
    pub(crate) image: egui::ColorImage,
    /// 逻辑尺寸（px）：预览排版与贴图都用这个口径。
    pub(crate) size: egui::Vec2,
    /// 从图顶到数学基线的逻辑距离（px）：行内混排靠它与文字基线对齐。
    pub(crate) baseline: f32,
}

fn font() -> Result<&'static MathFont, MathError> {
    static FONT: OnceLock<Result<MathFont, String>> = OnceLock::new();
    FONT.get_or_init(|| MathFont::stix_two_math().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| MathError(e.clone()))
}

/// Dim 是有理数，只在出图边界上转成 f32（crate 自己的光栅化也是这么干的）。
fn dim_f32(dim: &Dim) -> f32 {
    f32::from_bits(dim.to_ieee32_bits())
}

fn dim_from_f32(value: f32) -> Dim {
    Dim::from_ieee32_bits(value.to_bits())
}

/// 把一段 LaTeX 数学源码排成可贴图的位图。
///
/// `font_size_pt` 用预览的逻辑磅（与正文字号同一口径，96dpi 下 1pt = 96/72 px）；
/// `oversample` 是超采样倍率（2.0 时按 192dpi 出图，贴图时缩小一半，高分屏不糊）。
pub(crate) fn render(
    src: &str,
    display: bool,
    font_size_pt: f32,
    color: egui::Color32,
    oversample: f32,
) -> Result<RenderedMath, MathError> {
    let font = font()?;
    let ast = latex_rust::parse(src).map_err(|e| MathError(e.to_string()))?;
    let style = if display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let boxed = latex_rust::layout(&ast, font, style)?;

    let mut options = PngOptions::new();
    options.font_size_pt = dim_from_f32(font_size_pt);
    options.dpi = dim_from_f32(96.0 * oversample);
    options.color = MathColor::rgb(color.r(), color.g(), color.b());
    options.background = PngBackground::Transparent;
    options.display = display;
    // 复用上面排好的盒模型，不再让 latex_to_png 重复 parse + layout。
    let png = latex_rust::render_png(&boxed, font, &options)?;

    let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| MathError(format!("公式 PNG 解码失败：{e}")))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        decoded.as_raw(),
    );

    // 盒模型的 width/height/depth 单位是 em；em 在逻辑口径下 = font_size_pt * 96/72 px。
    // 光栅化画布就是 (width, height+depth) 紧贴盒模型、基线距顶 height（见 crate 的
    // raster.rs），所以逻辑尺寸与基线可以直接换算，不需要像素扫描。
    let em_px = font_size_pt * (96.0 / 72.0);
    let size = egui::vec2(
        dim_f32(&boxed.width) * em_px,
        dim_f32(&(&boxed.height + &boxed.depth)) * em_px,
    );
    let baseline = dim_f32(&boxed.height) * em_px;
    Ok(RenderedMath {
        image,
        size,
        baseline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: egui::Color32 = egui::Color32::BLACK;

    fn render_ok(src: &str, display: bool) -> RenderedMath {
        render(src, display, 14.0, BLACK, 2.0)
            .unwrap_or_else(|e| panic!("公式应渲染成功：{src}，实际失败：{e}"))
    }

    /// 研究报告里常见的构造都要能排出来。
    #[test]
    fn renders_common_constructions() {
        let inline = [
            r"E=mc^2",
            r"e^{i\pi}+1=0",
            r"\frac{a}{b}",
            r"\sqrt{x^2+y^2}",
            r"\alpha + \beta \leq \gamma",
            r"\vec{v} \cdot \hat{n}",
            r"\left( \frac{1}{2} \right)",
        ];
        for src in inline {
            let rendered = render_ok(src, false);
            assert!(
                rendered.size.x > 0.0 && rendered.size.y > 0.0,
                "{src} 尺寸应非零"
            );
        }

        let display = [
            r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
            r"\int_0^1 x^2 \, dx = \frac{1}{3}",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"\begin{cases} x + y = 1 \\ x - y = 2 \end{cases}",
            r"\begin{align} a &= b + c \\ &= d \end{align}",
            r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
        ];
        for src in display {
            let rendered = render_ok(src, true);
            assert!(
                rendered.size.x > 0.0 && rendered.size.y > 0.0,
                "{src} 尺寸应非零"
            );
        }
    }

    /// 位图与盒模型必须自洽：行内混排的基线对齐完全依赖这组数字。
    #[test]
    fn bitmap_matches_box_model() {
        let rendered = render_ok(r"x^2", false);
        // 光栅化画布紧贴盒模型（无内边距）：物理高 ≈ 逻辑高 × 超采样。
        let physical_expected = rendered.size.y * 2.0;
        let physical_actual = rendered.image.height() as f32;
        assert!(
            (physical_actual - physical_expected).abs() <= 1.0,
            "位图高 {physical_actual} 应贴合盒模型高 {physical_expected}"
        );
        assert!(
            rendered.baseline > 0.0 && rendered.baseline <= rendered.size.y,
            "基线应落在图内：{} / {}",
            rendered.baseline,
            rendered.size.y
        );
    }

    /// display 与 text 风格排出来的尺寸应不同（\sum 的上下标位置不一样）。
    #[test]
    fn display_style_changes_layout() {
        let src = r"\sum_{i=1}^{n} i";
        let inline = render_ok(src, false);
        let display = render_ok(src, true);
        assert!(
            (inline.size.y - display.size.y).abs() > 1.0,
            "display 与 text 的高度应有明显差别：{} vs {}",
            inline.size.y,
            display.size.y
        );
    }

    /// 不认识的命令必须报错而不是静默出假图，预览据此画占位框。
    #[test]
    fn unknown_command_is_an_error() {
        assert!(render(r"\notarealcommand{1}", false, 14.0, BLACK, 1.0).is_err());
    }

    /// 中文混排：STIX Two Math 没有中文字形，当前预期是报错（预览降级为占位框）。
    /// 若上游 crate 哪天支持了，这个测试会先红，到时把限制从文档里删掉。
    #[test]
    fn chinese_text_is_an_error() {
        assert!(render(r"\text{中文}", false, 14.0, BLACK, 1.0).is_err());
    }
}
