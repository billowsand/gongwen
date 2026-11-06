//! Word 插图用的 PDF → PNG：进程内渲染，输入及中间图均不落盘。
use anyhow::{Context, Result, ensure};
use hayro::{
    RenderCache, RenderSettings,
    hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf},
    vello_cpu::color::palette::css::WHITE,
};

pub(crate) fn png(bytes: &[u8], page: u32) -> Result<Vec<u8>> {
    let pdf = Pdf::new(bytes.to_vec())
        .map_err(|error| anyhow::anyhow!("无法解析 PDF 插图：{error:?}"))?;
    let index = page.checked_sub(1).context("PDF 页码必须从 1 开始")? as usize;
    let page = pdf.pages().get(index).context("PDF 插图页码超出范围")?;
    let (width, height) = page.render_dimensions();
    ensure!(
        width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0,
        "PDF 插图尺寸无效"
    );
    // 与旧 Poppler 路径同为 200 DPI；限制超大页面，避免解码炸弹。
    let scale = (200.0_f32 / 72.0)
        .min(8192.0 / width.max(height))
        .min((24_000_000.0 / (f64::from(width) * f64::from(height))).sqrt() as f32);
    let pixmap = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some((width * scale).max(1.0) as u16),
            bg_color: WHITE,
            ..Default::default()
        },
    );
    let image = image::RgbaImage::from_raw(
        u32::from(pixmap.width()),
        u32::from(pixmap.height()),
        pixmap.data_as_u8_slice().to_vec(),
    )
    .context("PDF 插图像素长度无效")?;
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut output, image::ImageFormat::Png)?;
    Ok(output.into_inner())
}
