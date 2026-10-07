//! 界面中的流程图复用 Mermaid 引擎和字体，不套用文档版心或纸面配色。

use super::*;
use eframe::egui;

/// 颜色依次为节点底、节点边框、文字、连线、分组底、分组边框。
/// 返回透明底纹理与原始逻辑尺寸；窗口缩放由界面按可用宽度处理。
pub(crate) fn render_interface(
    source: &str,
    colors: [egui::Color32; 6],
    pixels_per_point: f32,
    max_texture_side: usize,
) -> Result<(egui::ColorImage, egui::Vec2)> {
    check(source)?;
    let [fill, stroke, text, line, cluster, cluster_stroke] =
        colors.map(|color| format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b()));
    let family = Style::Research.font_family();
    let profile = HostThemeProfile::builder()
        .font_family(family)
        .font_size("14px")
        .roles(HostThemeRoles {
            canvas: Some("transparent".into()),
            surface: Some(fill.clone()),
            text: Some(text.clone()),
            border: Some(stroke.clone()),
            line: Some(line.clone()),
            edge_label_background: Some(cluster.clone()),
            cluster_background: Some(cluster.clone()),
            cluster_border: Some(cluster_stroke.clone()),
            ..HostThemeRoles::default()
        })
        .output(HostThemeOutput {
            scoped_css: Some(format!(
                ".node rect,.node polygon{{fill:{fill};stroke:{stroke};stroke-width:1px;}}\
                 text{{fill:{text};font-family:{family};font-size:14px!important;}}\
                 .flowchart-link{{stroke:{line};stroke-width:1.2px;}}\
                 marker path,.arrowheadPath{{fill:{line};stroke:{line};}}\
                 .edgeLabel rect{{fill:{cluster};}}\
                 .cluster rect{{fill:{cluster};stroke:{cluster_stroke};}}"
            )),
            ..HostThemeOutput::resvg_safe_editor()
        })
        .site_config("fontSize", 14)
        .site_config("fontFamily", family)
        .site_config("htmlLabels", false)
        .site_config(
            "flowchart",
            serde_json::json!({
                "htmlLabels": false, "curve": "linear", "nodeSpacing": 22,
                "rankSpacing": 24, "padding": 10, "diagramPadding": 8,
                "wrappingWidth": 200,
            }),
        )
        .build();
    let svg = HeadlessRenderer::new()
        .with_host_theme(&profile)
        .with_text_measurer(Arc::new(FontMeasurer {
            inner: VendoredFontMetricsTextMeasurer::default(),
        }))
        .with_strict_parsing()
        .render_svg_sync(source)
        .map_err(|error| anyhow!("流程图排版失败：{error}"))?
        .context("流程中没有图形")?;
    let tree = usvg::Tree::from_str(
        &svg,
        &usvg::Options {
            fontdb: fonts(),
            font_family: "FZHei-B01".into(),
            ..Default::default()
        },
    )
    .context("流程图无法光栅化")?;
    let size = tree.size();
    let logical_size = egui::vec2(size.width(), size.height());
    // 保留高分屏清晰度，并限制超长自定义流程的纹理尺寸。
    let limit = max_texture_side.clamp(1, 4096) as f32;
    let zoom = pixels_per_point
        .max(1.0)
        .min(limit / size.width().max(size.height()));
    let width = (size.width() * zoom).ceil().clamp(1.0, limit) as u32;
    let height = (size.height() * zoom).ceil().clamp(1.0, limit) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).context("流程图尺寸无效")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(zoom, zoom),
        &mut pixmap.as_mut(),
    );
    Ok((
        egui::ColorImage::from_rgba_premultiplied([width as usize, height as usize], pixmap.data()),
        logical_size,
    ))
}
