//! 研究报告公式的预览渲染：latex-rust 进程内排版 + PNG 光栅化。
//!
//! 不走 tectonic 往返——整文件编译是秒级延迟，跟不上逐帧预览；这里解析加光栅化
//! 都是微秒级。导出端仍以 tectonic + amsmath 为准，所以预览字形（STIX Two Math）
//! 与导出 PDF（CM 数学字体）不一致属预期，排版尺寸以导出为准。
//!
//! STIX Two Math 没有中文字形，`\text{中文}` 直接排会报缺字形。含中文的
//! `\text` 类命令（`\text`、`\mbox`、`\textrm`、`\textbf`、`\textit`、
//! `\textsf`、`\texttt`）在顶层出现时由本层拆出来，用随包仿宋单独光栅化，
//! 再按基线拼回位图；嵌在分数、上下标里的中文仍报错画占位框（导出不受影响）。

use eframe::egui;
use latex_rust::{
    BoxContent, Color as MathColor, Dim, MathBox, MathFont, MathStyle, PngBackground, PngOptions,
};
use resvg::{tiny_skia, usvg};
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

/// 随包仿宋：`\text{中文}` 拆出来后的渲染字体，与公文正文一脉。
struct CjkFont {
    db: std::sync::Arc<usvg::fontdb::Database>,
    family: String,
    data: Vec<u8>,
    ascender: f32,
    descender: f32,
    units_per_em: f32,
}

fn cjk_font() -> Result<&'static CjkFont, MathError> {
    static FONT: OnceLock<Option<CjkFont>> = OnceLock::new();
    FONT.get_or_init(|| {
        let dir = crate::portable_runtime::find_font_dir()?;
        let data = std::fs::read(dir.join("FangSong.ttf")).ok()?;
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(data.clone());
        let family = db.faces().next()?.families.first()?.0.clone();
        let face = ttf_parser::Face::parse(&data, 0).ok()?;
        let (ascender, descender, units_per_em) = (
            f32::from(face.ascender()),
            f32::from(face.descender()),
            f32::from(face.units_per_em()),
        );
        Some(CjkFont {
            db: std::sync::Arc::new(db),
            family,
            data,
            ascender,
            descender,
            units_per_em,
        })
    })
    .as_ref()
    .ok_or_else(|| MathError("随包中文字体不可用".into()))
}

/// 物理像素位图（超采样后、贴图前的口径），基线从图顶量起。
struct Bitmap {
    /// 预乘 RGBA，长度 = width * height * 4。
    premul: Vec<u8>,
    width: usize,
    height: usize,
    baseline: f32,
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
    let bitmap = if let Some(segments) = split_cjk_text(src) {
        render_segments(&segments, display, font_size_pt, color, oversample)?
    } else {
        render_math_bitmap(src, display, font_size_pt, color, oversample)?
    };
    Ok(RenderedMath {
        image: egui::ColorImage::from_rgba_unmultiplied(
            [bitmap.width, bitmap.height],
            &unpremul(&bitmap.premul),
        ),
        size: egui::vec2(bitmap.width as f32, bitmap.height as f32) / oversample,
        baseline: bitmap.baseline / oversample,
    })
}

/// 纯数学段：latex-rust 排版 + 光栅化 + 裁剪。
fn render_math_bitmap(
    src: &str,
    display: bool,
    font_size_pt: f32,
    color: egui::Color32,
    oversample: f32,
) -> Result<Bitmap, MathError> {
    let font = font()?;
    let ast = latex_rust::parse(src).map_err(|e| MathError(e.to_string()))?;
    let style = if display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let boxed = latex_rust::layout(&ast, font, style)?;
    let padded = pad_box(&boxed);

    let mut options = PngOptions::new();
    options.font_size_pt = dim_from_f32(font_size_pt);
    options.dpi = dim_from_f32(96.0 * oversample);
    options.color = MathColor::rgb(color.r(), color.g(), color.b());
    options.background = PngBackground::Transparent;
    options.display = display;
    // 复用上面排好的盒模型，不再让 latex_to_png 重复 parse + layout。
    let png = latex_rust::render_png(&padded, font, &options)?;

    let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| MathError(format!("公式 PNG 解码失败：{e}")))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let (width, height) = (width as usize, height as usize);

    // 物理像素下的盒模型范围：画布是 padded 盒（见 crate 的 raster.rs：画布紧贴
    // 盒模型、基线距顶 height），原盒在其中右移、下移各一个 pad。
    let em_phys = font_size_pt * (96.0 / 72.0) * oversample;
    let pad = dim_f32(&pad_em()) * em_phys;
    let box_left = pad;
    let box_right = pad + dim_f32(&boxed.width) * em_phys;
    let box_top = pad;
    let box_bottom = pad + dim_f32(&(&boxed.height + &boxed.depth)) * em_phys;
    let baseline_phys = pad + dim_f32(&boxed.height) * em_phys;

    // 裁剪区 = 盒模型 ∪ 实际墨迹：盒模型保住公式与文字的间距，墨迹兜住
    // 字形超出盒模型的部分（STIX 的升部、分子等常顶出盒顶）。
    let ink = ink_bounds(decoded.as_raw(), width, height);
    let mut left = box_left.floor() as usize;
    let mut right = (box_right.ceil() as usize).min(width);
    let mut top = box_top.floor() as usize;
    let mut bottom = (box_bottom.ceil() as usize).min(height);
    if let Some([ink_left, ink_top, ink_right, ink_bottom]) = ink {
        left = left.min(ink_left);
        top = top.min(ink_top);
        right = right.max(ink_right);
        bottom = bottom.max(ink_bottom);
    }
    let crop_w = right.saturating_sub(left).max(1);
    let crop_h = bottom.saturating_sub(top).max(1);
    // 裁出的区段连同基线一起平移回原点。
    let mut rgba = Vec::with_capacity(crop_w * crop_h * 4);
    for y in top..top + crop_h {
        let row = (y * width + left) * 4;
        rgba.extend_from_slice(&decoded.as_raw()[row..row + crop_w * 4]);
    }
    Ok(Bitmap {
        premul: premul(&rgba),
        width: crop_w,
        height: crop_h,
        baseline: baseline_phys - top as f32,
    })
}

/// 拆分后的公式段：数学部分走 STIX，含中文的 `\text` 类命令走仿宋。
enum Segment {
    Math(String),
    Cjk(String),
}

/// 可拆分的 `\text` 类命令。`\mathrm`、`\mathbf` 等数学字体命令保持原样
/// （中文出现在那里本来就是不规范的写法，报错画占位框）。
const TEXT_COMMANDS: &[&str] = &[
    "text", "mbox", "textrm", "textbf", "textit", "textsf", "texttt",
];

/// 在顶层把含中文的 `\text{...}` 拆成独立段。没有任何可拆段时返回 None，
/// 整体走纯数学渲染。拆不了的情况（嵌在分数/上下标里、中文段带着上下标）
/// 同样返回 None，维持原来的报错占位框。
fn split_cjk_text(src: &str) -> Option<Vec<Segment>> {
    let font = font().ok()?;
    let chars: Vec<char> = src.chars().collect();
    let mut segments: Vec<Segment> = Vec::new();
    let mut math = String::new();
    let mut depth = 0usize;
    let mut found_cjk = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && depth == 0 {
            let mut j = i + 1;
            let mut name = String::new();
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                name.push(chars[j]);
                j += 1;
            }
            if TEXT_COMMANDS.contains(&name.as_str()) {
                let mut k = j;
                while k < chars.len() && chars[k].is_whitespace() {
                    k += 1;
                }
                if k < chars.len() && chars[k] == '{' {
                    // 配平花括号取参数。
                    let mut arg = String::new();
                    let mut d = 0usize;
                    let mut m = k;
                    while m < chars.len() {
                        match chars[m] {
                            '{' => {
                                d += 1;
                                if d > 1 {
                                    arg.push('{');
                                }
                            }
                            '}' => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                                arg.push('}');
                            }
                            other => arg.push(other),
                        }
                        m += 1;
                    }
                    let followed_by_script = matches!(chars.get(m + 1), Some('^') | Some('_'));
                    let has_cjk = arg.chars().any(|ch| font.glyph(ch).is_err());
                    if m < chars.len() && has_cjk && !followed_by_script {
                        found_cjk = true;
                        if !math.trim().is_empty() {
                            segments.push(Segment::Math(std::mem::take(&mut math)));
                        }
                        segments.push(Segment::Cjk(arg));
                        i = m + 1;
                        continue;
                    }
                }
            }
            math.push(c);
            if name.is_empty() {
                // 反斜杠转义（`\{`、`\ ` 等）：被转义的字符原样带走。
                if j < chars.len() {
                    math.push(chars[j]);
                }
                i = (j + 1).min(chars.len());
            } else {
                // 普通命令原样保留，继续扫描命令名之后。
                math.push_str(&name);
                i = j;
            }
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        math.push(c);
        i += 1;
    }
    if !math.is_empty() {
        segments.push(Segment::Math(math));
    }
    found_cjk.then_some(segments)
}

/// 中文段：SVG `<text>` 走 usvg/resvg 光栅化，字体库只装随包仿宋，
/// 度量（字宽、基线）用 ttf-parser 按同一字体实测量，两边不会错位。
fn render_cjk_bitmap(
    text: &str,
    font_size_pt: f32,
    color: egui::Color32,
    oversample: f32,
) -> Result<Bitmap, MathError> {
    let font = cjk_font()?;
    let face = ttf_parser::Face::parse(&font.data, 0)
        .map_err(|e| MathError(format!("中文字体解析失败：{e}")))?;
    let advance = |ch: char| {
        face.glyph_index(ch)
            .and_then(|glyph| face.glyph_hor_advance(glyph))
            .map_or(font.units_per_em, f32::from)
            / font.units_per_em
    };
    let font_size_px = font_size_pt * (96.0 / 72.0) * oversample;
    let pad = (font_size_px * 0.25).ceil();
    let width = (text.chars().map(advance).sum::<f32>() * font_size_px + pad * 2.0)
        .ceil()
        .max(1.0) as u32;
    let ascender = font.ascender / font.units_per_em * font_size_px;
    let descender = -font.descender / font.units_per_em * font_size_px;
    let height = (ascender + descender + pad * 2.0).ceil().max(1.0) as u32;
    let baseline = pad + ascender;
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">\
         <text x=\"{pad}\" y=\"{baseline:.2}\" font-family=\"{}\" font-size=\"{font_size_px:.2}\" \
         fill=\"#{:02x}{:02x}{:02x}\">{escaped}</text></svg>",
        font.family,
        color.r(),
        color.g(),
        color.b()
    );
    let tree = usvg::Tree::from_str(
        &svg,
        &usvg::Options {
            fontdb: font.db.clone(),
            ..Default::default()
        },
    )
    .map_err(|e| MathError(format!("中文公式排版失败：{e}")))?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| MathError("中文公式画布无效".into()))?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    Ok(Bitmap {
        premul: pixmap.take(),
        width: width as usize,
        height: height as usize,
        baseline,
    })
}

/// 各段按基线拼成一张位图。段之间不加胶：TeX 里 `\text` 是 Ord 原子，
/// 与两侧数学原子的间距本来就是零，显式间距（`\ `）留在数学段里。
fn render_segments(
    segments: &[Segment],
    display: bool,
    font_size_pt: f32,
    color: egui::Color32,
    oversample: f32,
) -> Result<Bitmap, MathError> {
    let mut bitmaps = Vec::with_capacity(segments.len());
    for segment in segments {
        let bitmap = match segment {
            Segment::Math(src) => {
                // latex-rust 把行尾的控制空格（`\ `）当成悬空反斜杠报错，
                // 补一个空分组既保住间距又能解析。
                let padded;
                let src = if src.chars().nth_back(1).is_some_and(|c| c == '\\')
                    && src.chars().last().is_some_and(|c| c.is_whitespace())
                {
                    padded = format!("{src}{{}}");
                    &padded
                } else {
                    src
                };
                render_math_bitmap(src, display, font_size_pt, color, oversample)?
            }
            Segment::Cjk(text) => render_cjk_bitmap(text, font_size_pt, color, oversample)?,
        };
        bitmaps.push(bitmap);
    }
    let baseline = bitmaps
        .iter()
        .map(|bitmap| bitmap.baseline)
        .fold(0.0f32, f32::max);
    let height = bitmaps
        .iter()
        .map(|bitmap| baseline - bitmap.baseline + bitmap.height as f32)
        .fold(0.0f32, f32::max)
        .ceil() as usize;
    let width = bitmaps.iter().map(|bitmap| bitmap.width).sum::<usize>();
    let mut out = vec![0u8; width * height * 4];
    let mut x = 0usize;
    for bitmap in &bitmaps {
        let y = (baseline - bitmap.baseline).round() as usize;
        blit(
            &mut out,
            (width, height),
            &bitmap.premul,
            (bitmap.width, bitmap.height),
            (x, y),
        );
        x += bitmap.width;
    }
    Ok(Bitmap {
        premul: out,
        width,
        height,
        baseline,
    })
}

/// 预乘 alpha：直乘 RGBA → 预乘 RGBA。
fn premul(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let a = u32::from(px[3]);
        for channel in &px[0..3] {
            out.push(((u32::from(*channel) * a + 127) / 255) as u8);
        }
        out.push(px[3]);
    }
    out
}

/// 预乘 → 直乘，四舍五入回误差用 a/2 补偿；全透像素保持全零。
fn unpremul(premul: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(premul.len());
    for px in premul.chunks_exact(4) {
        let a = u32::from(px[3]);
        let restore = |channel: u8| -> u8 {
            let numer = u32::from(channel) * 255 + a / 2;
            numer.checked_div(a).unwrap_or(0) as u8
        };
        for channel in &px[0..3] {
            out.push(restore(*channel));
        }
        out.push(px[3]);
    }
    out
}

/// 预乘 RGBA 的 source-over 前景叠加。
fn blit(
    dst: &mut [u8],
    (dst_w, dst_h): (usize, usize),
    src: &[u8],
    (src_w, src_h): (usize, usize),
    (x_off, y_off): (usize, usize),
) {
    for y in 0..src_h {
        let dy = y_off + y;
        if dy >= dst_h {
            break;
        }
        for x in 0..src_w {
            let dx = x_off + x;
            if dx >= dst_w {
                break;
            }
            let s = (y * src_w + x) * 4;
            let d = (dy * dst_w + dx) * 4;
            let sa = u32::from(src[s + 3]);
            if sa == 0 {
                continue;
            }
            let inv = 255 - sa;
            for c in 0..3 {
                dst[d + c] =
                    (u32::from(src[s + c]) + (u32::from(dst[d + c]) * inv + 127) / 255) as u8;
            }
            dst[d + 3] = (sa + (u32::from(dst[d + 3]) * inv + 127) / 255) as u8;
        }
    }
}

/// 光栅化前四周留白的 em 数。latex-rust 的画布紧贴盒模型，而 STIX Two Math 的
/// 字形墨迹常顶出盒模型（`b` 的升部、`\frac` 的分子都会被切掉顶），所以先把盒子
/// 撑大再画，画完按实际墨迹裁回来。
fn pad_em() -> Dim {
    dim_from_f32(0.5)
}

/// 把排好的盒子包进一个四周各大 `pad_em` 的 HList：左边垫一段 kern，
/// 高度、深度、宽度各加留白。
fn pad_box(boxed: &MathBox) -> MathBox {
    let pad = pad_em();
    let kern = MathBox {
        width: pad.clone(),
        height: Dim::zero(),
        depth: Dim::zero(),
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::Kern(pad.clone()),
    };
    let mut inner = boxed.clone();
    inner.shift = Dim::zero();
    MathBox {
        width: &(&boxed.width + &pad) + &pad,
        height: &boxed.height + &pad,
        depth: &boxed.depth + &pad,
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::HList(vec![kern, inner]),
    }
}

/// RGBA 像素里 alpha 非零的外接框 `[left, top, right, bottom)`；全透明时为 None。
fn ink_bounds(rgba: &[u8], width: usize, height: usize) -> Option<[usize; 4]> {
    let mut bounds: Option<[usize; 4]> = None;
    for y in 0..height {
        for x in 0..width {
            if rgba[(y * width + x) * 4 + 3] == 0 {
                continue;
            }
            let b = bounds.get_or_insert([x, y, x + 1, y + 1]);
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x + 1);
            b[3] = b[3].max(y + 1);
        }
    }
    bounds
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
        // 裁剪后的位图：物理高 ≈ 逻辑高 × 超采样。
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

    /// 字形墨迹顶出盒模型时不能被切：留白后的画布四边都不该碰到墨迹，
    /// 否则说明 `pad_em` 不够。`b`、`\frac12` 在不留白时都会被切掉顶。
    #[test]
    fn padding_leaves_ink_unclipped() {
        let font = font().expect("STIX Two Math 应可加载");
        let mut options = PngOptions::new();
        options.font_size_pt = dim_from_f32(14.0);
        options.dpi = dim_from_f32(192.0);
        for (src, style) in [
            ("b", MathStyle::Text),
            (r"\frac12", MathStyle::Text),
            (r"\sum_a^b{a+b}=c", MathStyle::Text),
            (r"\int_0^1 f(x)\,dx", MathStyle::Display),
            (r"\left( \frac{1}{2} \right)", MathStyle::Text),
        ] {
            let ast = latex_rust::parse(src).expect("可解析");
            let boxed = latex_rust::layout(&ast, font, style).expect("可排版");
            let png = latex_rust::render_png(&pad_box(&boxed), font, &options).expect("可出图");
            let decoded = image::load_from_memory(&png).expect("可解码").to_rgba8();
            let (w, h) = decoded.dimensions();
            let [left, top, right, bottom] =
                ink_bounds(decoded.as_raw(), w as usize, h as usize).expect("应有墨迹");
            assert!(
                left > 0 && top > 0 && right < w as usize && bottom < h as usize,
                "{src}：墨迹 {left},{top}..{right},{bottom} 碰到了 {w}x{h} 画布边缘"
            );
        }
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

    /// 中文混排：含中文的顶层 `\text` 拆出用仿宋渲染，整式可出图且墨迹非空。
    /// 跑在没随包字体的环境（CI）时安静跳过。
    #[test]
    fn chinese_text_renders_with_fallback() {
        if cjk_font().is_err() {
            eprintln!("跳过：当前环境没有随包中文字体");
            return;
        }
        for src in [
            r"\text{中文}",
            r"\{x \mid x\in A \ \text{或}\ x\in B\}",
            r"\text{或}A\text{或}",
        ] {
            let rendered = render(src, false, 14.0, BLACK, 2.0)
                .unwrap_or_fail(&format!("中文公式应渲染成功：{src}"));
            assert!(
                rendered.size.x > 0.0 && rendered.size.y > 0.0,
                "{src} 尺寸应非零"
            );
            let has_ink = rendered.image.pixels.iter().any(|px| px.a() > 0);
            assert!(has_ink, "{src} 应有墨迹");
        }
        // 嵌在上下标、分数里的中文 \text 拆不出来，维持报错占位框（导出正常）。
        assert!(
            render(
                r"S = \underbrace{\frac{1}{2}ab}_{\text{三角形面积}}",
                false,
                14.0,
                BLACK,
                1.0
            )
            .is_err()
        );
    }

    /// 测试小工具：把渲染失败炸成带上下文的 panic。
    trait RenderExt {
        fn unwrap_or_fail(self, context: &str) -> RenderedMath;
    }

    impl RenderExt for Result<RenderedMath, MathError> {
        fn unwrap_or_fail(self, context: &str) -> RenderedMath {
            self.unwrap_or_else(|e| panic!("{context}，实际失败：{e}"))
        }
    }

    #[test]
    fn splits_top_level_cjk_text_only() {
        let names = |segments: Option<Vec<Segment>>| {
            segments
                .iter()
                .flatten()
                .map(|segment| match segment {
                    Segment::Math(s) => format!("M:{s}"),
                    Segment::Cjk(s) => format!("C:{s}"),
                })
                .collect::<Vec<_>>()
        };
        // 顶层中文 \text 拆成三段，两侧数学原样保留（含显式空格 `\ `）。
        assert_eq!(
            names(split_cjk_text(r"\{x \mid x\in A \ \text{或}\ x\in B\}")),
            vec![
                r"M:\{x \mid x\in A \ ".to_string(),
                "C:或".to_string(),
                r"M:\ x\in B\}".to_string(),
            ]
        );
        assert_eq!(
            names(split_cjk_text(r"\text{或}A\text{或}")),
            vec!["C:或".to_string(), "M:A".to_string(), "C:或".to_string()]
        );
        // 纯英文 \text 与纯数学不拆。
        assert!(split_cjk_text(r"\text{abc}").is_none());
        assert!(split_cjk_text(r"x^2+y^2").is_none());
        // 嵌在分组里、或中文段带着上下标，拆不了就维持整体报错占位框。
        assert!(split_cjk_text(r"{\text{或}}").is_none());
        assert!(split_cjk_text(r"\text{或}^2").is_none());
        // 转义花括号不干扰深度统计。
        assert!(split_cjk_text(r"\{ \text{或} \}").is_some());
    }
}
