//! Mermaid 图表的本机渲染与导出缓存。源码始终保留在 Markdown 中。
//!
//! 一张图的来路：引擎解析排版出 SVG（配色取自 [`DiagramTheme`]，字体随文种）→
//! 按实际字号把图摆到版心宽的画布上 → 用随包字体转 PNG（预览、Word）或 PDF（TeX）。
//! 各导出器照旧把图片铺满画布，图内文字因此总是设定的字号：小图不会被放大成大字，
//! 宽图、长图才整体缩小到版心以内。
//!
//! 图种按围栏首行路由到两个纯 Rust 引擎（都稳定版，不引浏览器）：
//! - merman 0.7：flowchart / graph（泳道图用 subgraph 分组表达）；
//! - mermaid-rs-renderer 0.2.2：sequenceDiagram、gantt、pie、sankey-beta、timeline、
//!   radar-beta。它的 SVG 再经 [`fixup_mmdr_colors`] 把写死的引擎调色板换成当前配色。

use crate::models::DiagramTheme;
use anyhow::{Context, Result, anyhow, bail};
use mdx::figure_size::{self, FigureSource};
use merman::render::{
    HeadlessRenderer, HostThemeOutput, HostThemeProfile, HostThemeRoles, TextMeasurer,
    VendoredFontMetricsTextMeasurer,
};
use merman_render::text::{TextMetrics, TextStyle, WrapMode};
use resvg::{tiny_skia, usvg};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const CACHE_DIR: &str = "mermaid-cache";
/// 配色、字体、画布算法或引擎版本变了都要改这里，旧缓存随之失效。
const STYLE_VERSION: &str = "gongwen-mermaid-v3/mmdr-0.2.2";

/// 排版用的字号（px）。两个引擎的内边距、节点间距是按 14–16px 的字定的，直接拿
/// 9pt 去排，框会显得空；先按 14px 排，落画布时再整体缩到目标字号。
const LAYOUT_FONT_PX: f64 = 14.0;
/// PNG 的分辨率：Word 与预览共用，打印不发虚。
const PNG_DPI: f64 = 300.0;
/// 公文版心（GB/T 9704）：156 × 225 mm，与研究报告的 `md2tex.cls` 同宽同高。
const OFFICIAL_TEXT_WIDTH_PT: f64 = 156.0 / 25.4 * 72.0;
const OFFICIAL_TEXT_HEIGHT_PT: f64 = 225.0 / 25.4 * 72.0;
/// 公文里一张图最多占版心高的这个比例，给图题和上下文留地方。
const OFFICIAL_MAX_HEIGHT_RATIO: f64 = 0.7;
const RESEARCH_TEXT_WIDTH_PT: f64 = figure_size::TEXT_WIDTH_MM / 25.4 * 72.0;
const RESEARCH_TEXT_HEIGHT_PT: f64 = figure_size::TEXT_HEIGHT_MM / 25.4 * 72.0;

/// 画图要用的随包字体：公文仿宋、研究报告方正黑体（老 runtime 没有时退到黑体），
/// 宋体兜住 GBK 生僻字。
const FONT_FILES: &[&str] = &["FangSong.ttf", "SimSun.ttf", "SimHei.ttf", "FZHei.ttf"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Official,
    Research,
}

impl Style {
    /// 图内文字：公文用仿宋，与正文一脉；研究报告用黑体，与图题、表头同一路。
    fn font_family(self) -> &'static str {
        match self {
            Self::Official => "FangSong_GB2312, SimSun, serif",
            Self::Research => "FZHei-B01, SimHei, sans-serif",
        }
    }

    /// 图内字号（pt）：公文小四，比三号正文低两档；研究报告小五，与图题同档。
    fn font_pt(self) -> f64 {
        match self {
            Self::Official => 12.0,
            Self::Research => 9.0,
        }
    }

    /// 节点边框与连线落到纸面上的线宽（pt）。
    fn stroke_pt(self) -> (f64, f64) {
        match self {
            Self::Official => (1.0, 0.9),
            Self::Research => (0.75, 0.65),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Png,
    Pdf,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }
}

/// 当前选用的配色，存 [`DiagramTheme::ALL`] 里的下标。预览在界面线程、导出在
/// 后台线程，都从这里取，与 `theme::set_current_paper` 同一种做法。
static THEME: AtomicU8 = AtomicU8::new(0);

pub(crate) fn set_theme(theme: DiagramTheme) {
    let index = DiagramTheme::ALL
        .iter()
        .position(|candidate| *candidate == theme)
        .unwrap_or(0);
    THEME.store(index as u8, Ordering::Relaxed);
}

fn current_theme() -> DiagramTheme {
    DiagramTheme::ALL
        .get(THEME.load(Ordering::Relaxed) as usize)
        .copied()
        .unwrap_or_default()
}

/// 「按文种自动」落到具体配色。
fn resolve(theme: DiagramTheme, style: Style) -> DiagramTheme {
    match (theme, style) {
        (DiagramTheme::Auto, Style::Official) => DiagramTheme::Ink,
        (DiagramTheme::Auto, Style::Research) => DiagramTheme::Navy,
        (theme, _) => theme,
    }
}

/// 一套配色。颜色只帮着分层次，不单独承载含义：黑白打印时靠线框和形状仍读得出。
/// 红色一概不用，留给红头、套红与修订标记。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Palette {
    /// 节点的填色、边框与文字。
    pub fill: &'static str,
    pub stroke: &'static str,
    pub text: &'static str,
    /// 连线与箭头，比边框略浅，让节点压得住线。
    pub line: &'static str,
    /// 判断（菱形）节点的填色：分支一眼就找得到。
    pub decision: &'static str,
    /// 子图（分组框）的底色与虚线框。
    pub cluster_fill: &'static str,
    pub cluster_stroke: &'static str,
    /// 分类色：饼图扇区、桑基节点、时间线分区、雷达曲线按出现顺序循环取色。
    /// 墨线/素灰给灰阶（黑白打印可辨），藏青/青瓷给同族色加米棕点缀。红色不用。
    pub category: &'static [&'static str; 8],
}

pub(crate) fn palette(theme: DiagramTheme, style: Style) -> Palette {
    match resolve(theme, style) {
        DiagramTheme::Auto | DiagramTheme::Ink => Palette {
            fill: "#FFFFFF",
            stroke: "#1A1A1A",
            text: "#111111",
            line: "#262626",
            decision: "#FFFFFF",
            cluster_fill: "#FFFFFF",
            cluster_stroke: "#808080",
            category: &[
                "#262626", "#4D4D4D", "#6E6E6E", "#8F8F8F", "#B0B0B0", "#5C5C5C", "#999999",
                "#CFCFCF",
            ],
        },
        DiagramTheme::Gray => Palette {
            fill: "#F2F2F2",
            stroke: "#4D4D4D",
            text: "#1A1A1A",
            line: "#5C5C5C",
            decision: "#E1E1E1",
            cluster_fill: "#FAFAFA",
            cluster_stroke: "#A6A6A6",
            category: &[
                "#404040", "#606060", "#808080", "#A0A0A0", "#C0C0C0", "#505050", "#909090",
                "#B0B0B0",
            ],
        },
        DiagramTheme::Navy => Palette {
            fill: "#EAF0F7",
            stroke: "#1F3A5F",
            text: "#15263D",
            line: "#3E5674",
            decision: "#FBF2DE",
            cluster_fill: "#F6F8FB",
            cluster_stroke: "#8EA3BB",
            category: &[
                "#1F3A5F", "#3E5674", "#5F7BA0", "#8EA3BB", "#B9C8D8", "#8A7B52", "#B4A67F",
                "#D8CC9F",
            ],
        },
        DiagramTheme::Celadon => Palette {
            fill: "#E6F1ED",
            stroke: "#2E6A5A",
            text: "#15302A",
            line: "#4A7668",
            decision: "#F6F2E6",
            cluster_fill: "#F4F8F6",
            cluster_stroke: "#90B3A7",
            category: &[
                "#2E6A5A", "#4A7668", "#6FA394", "#9DC2B4", "#C9DED4", "#8A7B52", "#B4A67F",
                "#D8CC9F",
            ],
        },
    }
}

/// Mermaid 围栏后的可选图题；研究报告的锚点也从这一行剥出。
pub(crate) fn caption(line: &str) -> Option<(&str, Option<&str>)> {
    let text = line.trim();
    let text = text
        .strip_prefix("图：")
        .or_else(|| text.strip_prefix("图:"))?;
    let (text, label) = crate::export::crossref::split_label(text.trim());
    Some((text.trim(), label))
}

pub(crate) fn is_open(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case("```mermaid")
}

pub(crate) fn is_close(line: &str) -> bool {
    line.trim() == "```"
}

/// 按随包字体的真实字宽量文字。merman 自带的量法照浏览器常见西文字体的字宽表估，
/// 对仿宋、方正黑体估得不准：框小了字会撑出菱形，框大了又空。宽度改用画图时
/// 同一支字体的字宽，行高、折行仍交给 merman。
struct FontMeasurer {
    inner: VendoredFontMetricsTextMeasurer,
}

impl FontMeasurer {
    fn width(&self, text: &str, style: &TextStyle) -> f64 {
        let fonts = fonts();
        let names: Vec<&str> = style
            .font_family
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(|name| name.trim().trim_matches(['"', '\'']))
            .filter(|name| !name.is_empty())
            .collect();
        let families: Vec<usvg::fontdb::Family> = names
            .iter()
            .map(|name| match *name {
                "serif" => usvg::fontdb::Family::Serif,
                "sans-serif" => usvg::fontdb::Family::SansSerif,
                name => usvg::fontdb::Family::Name(name),
            })
            .chain([usvg::fontdb::Family::Serif])
            .collect();
        let face = fonts.query(&usvg::fontdb::Query {
            families: &families,
            ..usvg::fontdb::Query::default()
        });
        // 缺字按全角一个字、半角半个字估，与浏览器退回后备字体时相差不大。
        let fallback = |ch: char| if ch.is_ascii() { 0.5 } else { 1.0 };
        let ems = |line: &str| -> f64 {
            face.and_then(|id| {
                fonts.with_face_data(id, |data, index| {
                    let face = ttf_parser::Face::parse(data, index).ok()?;
                    let unit = f64::from(face.units_per_em());
                    Some(
                        line.chars()
                            .map(|ch| {
                                face.glyph_index(ch)
                                    .and_then(|glyph| face.glyph_hor_advance(glyph))
                                    .map_or(fallback(ch), |advance| f64::from(advance) / unit)
                            })
                            .sum::<f64>(),
                    )
                })?
            })
            .unwrap_or_else(|| line.chars().map(fallback).sum())
        };
        text.lines().map(ems).fold(0.0, f64::max) * style.font_size
    }
}

impl TextMeasurer for FontMeasurer {
    fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics {
        TextMetrics {
            width: self.width(text, style),
            ..self.inner.measure(text, style)
        }
    }

    fn measure_svg_text_computed_length_px(&self, text: &str, style: &TextStyle) -> f64 {
        self.width(text, style)
    }

    fn measure_svg_text_bbox_x(&self, text: &str, style: &TextStyle) -> (f64, f64) {
        let half = self.width(text, style) / 2.0;
        (half, half)
    }

    fn measure_svg_text_bbox_x_with_ascii_overhang(
        &self,
        text: &str,
        style: &TextStyle,
    ) -> (f64, f64) {
        self.measure_svg_text_bbox_x(text, style)
    }

    fn measure_svg_title_bbox_x(&self, text: &str, style: &TextStyle) -> (f64, f64) {
        self.measure_svg_text_bbox_x(text, style)
    }

    fn measure_svg_simple_text_bbox_width_px(&self, text: &str, style: &TextStyle) -> f64 {
        self.width(text, style)
    }

    fn measure_svg_raw_text_bbox_width_px(&self, text: &str, style: &TextStyle) -> f64 {
        self.width(text, style)
    }

    fn measure_svg_simple_text_bbox_width_for_wrap_px(&self, text: &str, style: &TextStyle) -> f64 {
        self.width(text, style)
    }

    fn measure_svg_simple_text_bbox_height_px(&self, text: &str, style: &TextStyle) -> f64 {
        self.inner
            .measure_svg_simple_text_bbox_height_px(text, style)
    }

    fn measure_wrapped(
        &self,
        text: &str,
        style: &TextStyle,
        max_width: Option<f64>,
        wrap_mode: WrapMode,
    ) -> TextMetrics {
        let metrics = self
            .inner
            .measure_wrapped(text, style, max_width, wrap_mode);
        // merman 折了行时各行的宽度量不回来，按限宽封顶。
        let width = self.width(text, style);
        TextMetrics {
            width: max_width.map_or(width, |limit| width.min(limit)),
            ..metrics
        }
    }

    fn measure_wrapped_with_raw_width(
        &self,
        text: &str,
        style: &TextStyle,
        max_width: Option<f64>,
        wrap_mode: WrapMode,
    ) -> (TextMetrics, Option<f64>) {
        (
            self.measure_wrapped(text, style, max_width, wrap_mode),
            Some(self.width(text, style)),
        )
    }

    fn measure_wrapped_raw(
        &self,
        text: &str,
        style: &TextStyle,
        max_width: Option<f64>,
        wrap_mode: WrapMode,
    ) -> TextMetrics {
        self.measure_wrapped(text, style, max_width, wrap_mode)
    }
}

fn renderer(style: Style, theme: DiagramTheme) -> HeadlessRenderer {
    let palette = palette(theme, style);
    // 线宽按落到纸面上的 pt 定，换算回排版时的 px。
    let scale = style.font_pt() / LAYOUT_FONT_PX;
    let (node_pt, line_pt) = style.stroke_pt();
    let node = node_pt / scale;
    let line = line_pt / scale;
    let Palette {
        fill,
        stroke,
        text,
        line: line_color,
        decision,
        cluster_fill,
        cluster_stroke,
        category: _,
    } = palette;
    // merman 按 SVG 根元素的 id 给这些规则加作用域，排在 Mermaid 自带样式之后，
    // 同权重下后写的生效。
    let css = format!(
        ".node rect,.node circle,.node ellipse,.node polygon,.node path\
         {{fill:{fill};stroke:{stroke};stroke-width:{node:.2}px;}}\
         .node polygon{{fill:{decision};}}\
         .flowchart-link,.edgePath .path{{stroke:{line_color};stroke-width:{line:.2}px;}}\
         .marker,.marker.cross{{fill:{line_color};stroke:{line_color};}}\
         .arrowheadPath{{fill:{line_color};}}\
         .edgeLabel rect{{opacity:1;fill:#FFFFFF;}}\
         .cluster rect{{fill:{cluster_fill};stroke:{cluster_stroke};\
         stroke-width:{line:.2}px;stroke-dasharray:6 3;}}\
         text{{fill:{text};}}\
         .cluster-label text,.cluster text{{fill:{stroke};}}"
    );
    let theme = HostThemeProfile::builder()
        .font_family(style.font_family())
        .font_size(format!("{LAYOUT_FONT_PX}px"))
        .roles(HostThemeRoles {
            canvas: Some("#FFFFFF".into()),
            surface: Some(fill.into()),
            text: Some(text.into()),
            border: Some(stroke.into()),
            line: Some(line_color.into()),
            edge_label_background: Some("#FFFFFF".into()),
            cluster_background: Some(cluster_fill.into()),
            cluster_border: Some(cluster_stroke.into()),
            ..HostThemeRoles::default()
        })
        .output(HostThemeOutput {
            scoped_css: Some(css),
            ..HostThemeOutput::resvg_safe_editor()
        })
        // 比 Mermaid 默认的 50 / 50 / 15 收紧：版心窄，图要紧凑。连线用圆角折线，
        // 比默认的样条曲线规整、像手绘的流程图；子图标题下多留一点，进来的箭头
        // 不压字。
        .site_config("htmlLabels", false)
        .site_config(
            "flowchart",
            serde_json::json!({
                "htmlLabels": false,
                "curve": "rounded",
                "nodeSpacing": 30,
                "rankSpacing": 38,
                "padding": 8,
                "diagramPadding": 6,
                "subGraphTitleMargin": { "top": 4, "bottom": 10 },
            }),
        )
        .build();
    HeadlessRenderer::new()
        .with_host_theme(&theme)
        .with_text_measurer(Arc::new(FontMeasurer {
            inner: VendoredFontMetricsTextMeasurer::default(),
        }))
        .with_strict_parsing()
}

/// 围栏首行决定走哪个引擎。merman 0.7 只认流程图；其余图种由
/// mermaid-rs-renderer 渲染，它的主题字段在 Rust 里配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineKind {
    Merman,
    Mmdr,
}

/// 图头：第一个非空、非 `%%` 注释行的首词；空围栏返回空串。
fn header(source: &str) -> &str {
    source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"))
        .map(|line| line.split_whitespace().next().unwrap_or(""))
        .unwrap_or("")
}

/// 按图头选引擎；不认识的图头返回 `None`。
fn engine_kind(source: &str) -> Option<EngineKind> {
    match header(source) {
        "flowchart" | "graph" => Some(EngineKind::Merman),
        "sequenceDiagram" | "gantt" | "pie" | "sankey-beta" | "timeline" | "radar-beta" => {
            Some(EngineKind::Mmdr)
        }
        _ => None,
    }
}

/// 不让单张图改版式、引外部资源；只放行已接入引擎的图种。
fn check(source: &str) -> Result<()> {
    if engine_kind(source).is_none() {
        bail!(
            "暂不支持的 Mermaid 图种「{}」：目前支持 flowchart/graph（含泳道分组）、\
             sequenceDiagram、gantt、pie、sankey-beta、timeline、radar-beta",
            header(source)
        );
    }
    if source.contains("%%{") {
        bail!("Mermaid 图内暂不支持 %%{{init}}%% 配置指令，样式在「图表样式」里统一设置");
    }
    for line in source.lines().map(str::trim) {
        if line == "click" || line.starts_with("click ") {
            bail!("Mermaid 图内暂不支持 click 点击链接");
        }
        // 只拦 HTML 标签；`<-->` 双向箭头这类连线写法照常放行。
        let tag = line.char_indices().any(|(index, ch)| {
            ch == '<'
                && line[index + 1..]
                    .chars()
                    .next()
                    .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!'))
        });
        if tag {
            bail!("Mermaid 图内暂不支持 HTML 标签：{line}");
        }
    }
    Ok(())
}

/// 图在画布上的摆法，单位 pt：画布宽高，以及排版 px 到 pt 的缩放比例。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    width: f64,
    height: f64,
    scale: f64,
}

fn placement(style: Style, width: f64, height: f64) -> Placement {
    let natural = style.font_pt() / LAYOUT_FONT_PX;
    match style {
        // 公文 TeX 写 `width=\textwidth`、Word 按版心宽封顶，画布恒为版心宽。
        Style::Official => {
            let scale = natural
                .min(OFFICIAL_TEXT_WIDTH_PT / width)
                .min(OFFICIAL_MAX_HEIGHT_RATIO * OFFICIAL_TEXT_HEIGHT_PT / height);
            Placement {
                width: OFFICIAL_TEXT_WIDTH_PT,
                height: height * scale,
                scale,
            }
        }
        Style::Research => research_placement(width, height, natural),
    }
}

/// 研究报告的插图由 mdx 按 `figure_size` 分四档定宽。画布宽取某一档、且 mdx 对这块
/// 画布算出的恰好就是这一档时，图就原样大小落到纸上。从原始大小往下找第一个成立的
/// 缩放，同一缩放下先试窄档，画布两侧的留白少一些。
fn research_placement(width: f64, height: f64, natural: f64) -> Placement {
    let mut scale = natural.min(RESEARCH_TEXT_WIDTH_PT / width);
    while scale > natural * 0.2 {
        let (drawn, tall) = (width * scale, height * scale);
        for step in figure_size::WIDTH_STEPS.iter().rev() {
            let canvas = step * RESEARCH_TEXT_WIDTH_PT;
            if canvas + 1e-6 >= drawn && lands_on(canvas, tall, *step) {
                return Placement {
                    width: canvas,
                    height: tall,
                    scale,
                };
            }
        }
        scale *= 0.98;
    }
    // 找不到（极端的长条图）就给满宽画布，交给 mdx 按比例缩放。
    let scale = natural
        .min(RESEARCH_TEXT_WIDTH_PT / width)
        .min(figure_size::MAX_HEIGHT_RATIO * RESEARCH_TEXT_HEIGHT_PT / height);
    Placement {
        width: RESEARCH_TEXT_WIDTH_PT,
        height: height * scale,
        scale,
    }
}

/// mdx 对这块画布算出的宽度是否正好是 `step` 档。高度上下浮动 1% 也不变才算，
/// 免得 PNG 取整像素后被推到邻档，Word 与 PDF 里的图大小不一。
fn lands_on(width: f64, height: f64, step: f64) -> bool {
    [0.99, 1.0, 1.01].iter().all(|factor| {
        let fraction = figure_size::width_fraction(FigureSource::Vector {
            width,
            height: height * factor,
        });
        (fraction - step).abs() < 1e-6
    })
}

/// SVG 根元素 `viewBox` 的宽高。
fn view_box(svg: &str) -> Option<(f64, f64)> {
    let start = svg.find("viewBox=\"")? + "viewBox=\"".len();
    let end = start + svg[start..].find('"')?;
    let numbers: Vec<f64> = svg[start..end]
        .split([' ', ','])
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    match numbers.as_slice() {
        [_, _, width, height] if *width > 0.0 && *height > 0.0 => Some((*width, *height)),
        _ => None,
    }
}

/// 去掉开标签里的一个属性（连同前面的空格）。
fn strip_attribute(tag: &str, name: &str) -> String {
    let needle = format!(" {name}=\"");
    let Some(start) = tag.find(&needle) else {
        return tag.to_string();
    };
    let value = start + needle.len();
    let Some(end) = tag[value..].find('"') else {
        return tag.to_string();
    };
    format!("{}{}", &tag[..start], &tag[value + end + 1..])
}

/// 把 merman 的 SVG 按 `placement` 嵌进画布：水平居中、贴顶。原根元素保留 id，
/// 它的样式表按这个 id 限定作用域。
fn compose(svg: &str, placement: Placement, background: Option<&str>) -> Result<String> {
    let (width, height) = view_box(svg).context("Mermaid 输出缺少 viewBox")?;
    let start = svg.find("<svg").context("Mermaid 输出不是 SVG")?;
    let end = start + svg[start..].find('>').context("Mermaid 输出不是 SVG")?;
    let mut root = svg[start..end].to_string();
    for name in ["width", "height", "style", "x", "y"] {
        root = strip_attribute(&root, name);
    }
    let (drawn_width, drawn_height) = (width * placement.scale, height * placement.scale);
    let x = (placement.width - drawn_width) / 2.0;
    let nested = root.replacen(
        "<svg",
        &format!(
            "<svg x=\"{x:.3}\" y=\"0\" width=\"{drawn_width:.3}\" height=\"{drawn_height:.3}\""
        ),
        1,
    );
    let background = background
        .map(|color| {
            format!(
                "<rect width=\"{:.3}\" height=\"{:.3}\" fill=\"{color}\"/>",
                placement.width, placement.height
            )
        })
        .unwrap_or_default();
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{w:.3}\" height=\"{h:.3}\" viewBox=\"0 0 {w:.3} {h:.3}\">{background}{nested}{rest}</svg>",
        w = placement.width,
        h = placement.height,
        rest = &svg[end..],
    ))
}

/// 画图用的字体库：随包字体只读一次，之后每张图共用。找不到随包字体（开发环境
/// 没放 runtime）才退到系统字体。
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut database = usvg::fontdb::Database::new();
            match crate::portable_runtime::find_font_dir() {
                Some(dir) => {
                    for file in FONT_FILES {
                        let _ = database.load_font_file(dir.join(file));
                    }
                }
                None => database.load_system_fonts(),
            }
            database.set_serif_family("SimSun");
            database.set_sans_serif_family("SimHei");
            Arc::new(database)
        })
        .clone()
}

fn to_png(svg: &str) -> Result<Vec<u8>> {
    let options = usvg::Options {
        fontdb: fonts(),
        font_family: "SimSun".into(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &options)
        .map_err(|error| anyhow!("Mermaid 图形无法光栅化：{error}"))?;
    let zoom = (PNG_DPI / 72.0) as f32;
    let size = tree.size();
    let mut pixmap = tiny_skia::Pixmap::new(
        (size.width() * zoom).ceil() as u32,
        (size.height() * zoom).ceil() as u32,
    )
    .context("Mermaid 图形尺寸无效")?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(zoom, zoom),
        &mut pixmap.as_mut(),
    );
    pixmap
        .encode_png()
        .map_err(|error| anyhow!("Mermaid PNG 编码失败：{error}"))
}

fn to_pdf(svg: &str) -> Result<Vec<u8>> {
    let options = svg2pdf::usvg::Options {
        fontdb: fonts(),
        font_family: "SimSun".into(),
        ..svg2pdf::usvg::Options::default()
    };
    let tree = svg2pdf::usvg::Tree::from_str(svg, &options)
        .map_err(|error| anyhow!("Mermaid 图形无法转 PDF：{error}"))?;
    // 默认 72 DPI：画布的 1 个单位就是 1 pt。
    svg2pdf::to_pdf(
        &tree,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions::default(),
    )
    .map_err(|error| anyhow!("Mermaid PDF 渲染失败：{error}"))
}

/// 把当前配色翻译成 mermaid-rs-renderer 的 Theme。甘特、时间线、饼图直接读这些
/// 字段；字号固定排版字号，落画布时再整体缩到目标字号（与 merman 路径一致）。
fn mmdr_theme(palette: Palette, style: Style) -> mermaid_rs_renderer::Theme {
    let mut theme = mermaid_rs_renderer::Theme::modern();
    theme.font_family = style.font_family().to_string();
    theme.font_size = LAYOUT_FONT_PX as f32;
    theme.background = "#FFFFFF".to_string();
    theme.primary_color = palette.fill.to_string();
    theme.primary_border_color = palette.stroke.to_string();
    theme.primary_text_color = palette.text.to_string();
    theme.secondary_color = palette.decision.to_string();
    theme.tertiary_color = palette.fill.to_string();
    theme.text_color = palette.text.to_string();
    theme.line_color = palette.line.to_string();
    theme.edge_label_background = "#FFFFFF".to_string();
    theme.cluster_background = palette.cluster_fill.to_string();
    theme.cluster_border = palette.cluster_stroke.to_string();
    theme.sequence_actor_fill = palette.fill.to_string();
    theme.sequence_actor_border = palette.stroke.to_string();
    theme.sequence_actor_line = palette.line.to_string();
    theme.sequence_note_fill = palette.decision.to_string();
    theme.sequence_note_border = palette.cluster_stroke.to_string();
    theme.sequence_activation_fill = palette.fill.to_string();
    theme.sequence_activation_border = palette.stroke.to_string();
    // 饼图扇区按分类色循环；填色本来就浅，不再叠透明度，打印不发灰。
    let mut pie_colors: [String; 12] =
        std::array::from_fn(|index| palette.category[index % palette.category.len()].to_string());
    pie_colors[..palette.category.len()].clone_from_slice(&palette.category.map(str::to_string));
    theme.pie_colors = pie_colors;
    theme.pie_opacity = 1.0;
    theme.pie_stroke_color = palette.stroke.to_string();
    theme.pie_outer_stroke_color = palette.stroke.to_string();
    theme.pie_title_text_color = palette.text.to_string();
    theme.pie_section_text_color = palette.text.to_string();
    theme.pie_legend_text_color = palette.text.to_string();
    theme.pie_title_text_size = 16.0;
    theme.pie_section_text_size = 12.0;
    theme.pie_legend_text_size = 12.0;
    theme
}

/// mermaid-rs-renderer 0.2.2 的桑基节点色与雷达曲线色是引擎写死的调色板
/// （含红/粉色，不合公文配色），按出现顺序整串替换成当前配色的分类色。
/// 常量在锁定的 0.2.2 里是固定的；引擎升级时要重新核对这一段。
fn fixup_mmdr_colors(svg: &str, source: &str, palette: Palette) -> String {
    let mut out = svg.to_string();
    match header(source) {
        "sankey-beta" => {
            const SANKEY_PALETTE: [&str; 10] = [
                "#4e79a7", "#f28e2c", "#e15759", "#76b7b2", "#59a14f", "#edc949", "#af7aa1",
                "#ff9da7", "#9c755f", "#bab0ab",
            ];
            for (index, color) in SANKEY_PALETTE.iter().enumerate() {
                out = out.replace(color, palette.category[index % palette.category.len()]);
            }
            // 引擎默认链路半透明（0.5），白底上洗得发灰；配色本身已够轻，加深到 0.85。
            out = out.replace("stroke-opacity=\"0.5\"", "stroke-opacity=\"0.85\"");
        }
        "radar-beta" => {
            const RADAR_HUES: [i32; 12] = [240, 60, 80, 270, 300, 330, 0, 30, 90, 150, 180, 210];
            for (index, hue) in RADAR_HUES.iter().enumerate() {
                let needle = format!("hsl({hue}, 100%, 76.2745098039%)");
                out = out.replace(&needle, palette.category[index % palette.category.len()]);
            }
        }
        _ => {}
    }
    out
}

/// mmdr 量宽走系统 fontdb：量宽时按字体族链落到系统里同度量的一支（Windows 的
/// 仿宋/黑体、Linux 的 serif/sans CJK），渲染时 resvg 用随包字体。中西文都是
/// 全宽/半宽等宽推进，两边只差拉丁字形的几个百分点，框内留白足够吸收。
fn render_mmdr(source: &str, style: Style, theme: DiagramTheme) -> Result<String> {
    let palette = palette(theme, style);
    let options = mermaid_rs_renderer::RenderOptions {
        theme: mmdr_theme(palette, style),
        ..Default::default()
    };
    let svg = mermaid_rs_renderer::render_with_options(source, options)
        .map_err(|error| anyhow!("Mermaid 解析或排版失败：{error}"))?;
    Ok(fixup_mmdr_colors(&svg, source, palette))
}

fn render(source: &str, style: Style, theme: DiagramTheme, format: Format) -> Result<Vec<u8>> {
    check(source)?;
    let svg = match engine_kind(source) {
        Some(EngineKind::Merman) => renderer(style, theme)
            .render_svg_sync(source)
            .map_err(|error| anyhow!("Mermaid 解析或排版失败：{error}"))?
            .context("Mermaid 围栏里没有图形")?,
        Some(EngineKind::Mmdr) => render_mmdr(source, style, theme)?,
        None => unreachable!("check 已拦下不支持的图种"),
    };
    let (width, height) = view_box(&svg).context("Mermaid 输出缺少 viewBox")?;
    let placement = placement(style, width, height);
    match format {
        Format::Png => to_png(&compose(&svg, placement, Some("#FFFFFF"))?),
        Format::Pdf => to_pdf(&compose(&svg, placement, None)?),
    }
}

/// 画不出来的图记下错误：预览每帧都会来问，改好源码之前不必每帧重排一遍。
fn failures() -> &'static Mutex<HashMap<String, String>> {
    static FAILURES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    FAILURES.get_or_init(Mutex::default)
}

fn cache_at(
    base: &Path,
    source: &str,
    style: Style,
    theme: DiagramTheme,
    format: Format,
) -> Result<String> {
    let theme = resolve(theme, style);
    let mut hash = Sha256::new();
    for part in [
        STYLE_VERSION,
        &format!("{style:?}"),
        &format!("{theme:?}"),
        source,
    ] {
        hash.update(part);
        hash.update([0]);
    }
    let digest = hash.finalize();
    let name = format!(
        "{}.{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        format.extension()
    );
    let relative = format!("{CACHE_DIR}/{name}");
    let path = base.join(&relative);
    if path.is_file() {
        return Ok(relative);
    }
    let lock = || {
        failures()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    };
    if let Some(error) = lock().get(&relative) {
        bail!("{error}");
    }
    let bytes = match render(source, style, theme, format) {
        Ok(bytes) => bytes,
        Err(error) => {
            let mut failures = lock();
            // 编辑中途的半截源码会攒下不少条，够多就整个清掉重记。
            if failures.len() >= 256 {
                failures.clear();
            }
            failures.insert(relative, format!("{error:#}"));
            return Err(error);
        }
    };
    fs::create_dir_all(path.parent().expect("缓存文件有父目录"))?;
    // 先写临时文件再改名：预览线程与导出线程可能同时生成同一张图，另一方不能
    // 读到写了一半的文件。
    let partial = path.with_extension(format!("{}.part", std::process::id()));
    fs::write(&partial, bytes)
        .with_context(|| format!("无法写入 Mermaid 图：{}", partial.display()))?;
    if fs::rename(&partial, &path).is_err() {
        let _ = fs::remove_file(&partial);
        if !path.is_file() {
            bail!("无法写入 Mermaid 图：{}", path.display());
        }
    }
    Ok(relative)
}

/// 按当前配色生成（或取缓存）一张图，返回相对用户目录的路径。
pub(crate) fn cache(source: &str, style: Style, format: Format) -> Result<String> {
    cache_at(
        &crate::storage::config_dir()?,
        source,
        style,
        current_theme(),
        format,
    )
}

/// 将围栏临时换为图片引用，让已有公文与 mdx 导出器继续排版；不修改用户正文。
pub(crate) fn materialize(markdown: &str, style: Style, format: Format) -> Result<String> {
    if !markdown.lines().any(is_open) {
        return Ok(markdown.to_string());
    }
    materialize_at(
        &crate::storage::config_dir()?,
        markdown,
        style,
        current_theme(),
        format,
    )
}

fn materialize_at(
    base: &Path,
    markdown: &str,
    style: Style,
    theme: DiagramTheme,
    format: Format,
) -> Result<String> {
    if !markdown.lines().any(is_open) {
        return Ok(markdown.to_string());
    }
    let lines: Vec<&str> = markdown.lines().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < lines.len() {
        if !is_open(lines[index]) {
            out.push_str(lines[index]);
            out.push('\n');
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let mut source = Vec::new();
        while index < lines.len() && !is_close(lines[index]) {
            source.push(lines[index]);
            index += 1;
        }
        if index == lines.len() {
            bail!("第 {} 行的 Mermaid 围栏没有结束标记", start + 1);
        }
        index += 1;
        let source = source.join("\n");
        let label = lines.get(index).and_then(|line| caption(line));
        let (title, anchor) = label.unwrap_or(("", None));
        if label.is_some() {
            index += 1;
        }
        let path = cache_at(base, &source, style, theme, format)
            .with_context(|| format!("第 {} 行的 Mermaid 图渲染失败", start + 1))?;
        match style {
            Style::Research => {
                out.push_str(&format!("![{title}]({path})"));
                if let Some(anchor) = anchor {
                    out.push_str(&format!("{{#{anchor}}}"));
                }
                out.push('\n');
            }
            Style::Official => {
                out.push_str(&format!("![]({path})\n"));
                if !title.is_empty() {
                    // 居中区到空行为止：图题后补一个空行，紧跟着写的正文不会被一起居中。
                    out.push_str(&format!("\n<!-- [居中] -->\n图：{title}\n\n"));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "flowchart TD\n S([收文]) --> A[登记编号]\n A --> B{是否需要会签}\n B -- 是 --> C[相关单位会签]\n B -- 否 --> D[审核]\n C --> D\n subgraph 签发环节\n D --> E[领导签发]\n E --> F[(归档)]\n end\n F --> G((办结))";

    /// 本期接入的全部图种：一份能代表该图种最小语法的中文样例。
    const KIND_SAMPLES: &[(&str, &str)] = &[
        ("flowchart", SAMPLE),
        (
            "sequence",
            "sequenceDiagram\n  经办人->>处长: 提交请示\n  处长-->>经办人: 退回修改\n  Note over 处长: 研究\n  loop 修改\n    经办人->>处长: 再次报送\n  end",
        ),
        (
            "gantt",
            "gantt\n  title 项目进度\n  dateFormat YYYY-MM-DD\n  section 调研\n  收集资料 :a1, 2026-10-01, 10d\n  撰写报告 :a2, after a1, 7d\n  section 审定\n  内部评审 :a3, after a2, 5d",
        ),
        (
            "pie",
            "pie title 经费构成\n  \"人员经费\" : 45\n  \"公用经费\" : 30\n  \"项目支出\" : 25",
        ),
        (
            "sankey",
            "sankey-beta\n  财政预算,人员经费,45\n  财政预算,公用经费,30\n  财政预算,项目支出,25",
        ),
        (
            "timeline",
            "timeline\n  title 工作规划\n  section 2026年 第四季度\n    完成调研 : 形成调研报告\n  section 2027年 上半年\n    出台办法 : 印发实施",
        ),
        (
            "radar",
            "radar-beta\n  axis 政治素质, 业务能力, 工作作风, 创新意识\n  curve 部门甲 {0.9, 0.8, 0.85, 0.7}\n  curve 部门乙 {0.7, 0.9, 0.75, 0.85}",
        ),
    ];

    #[test]
    fn native_flowchart_renders_both_formats() {
        let source = "flowchart LR\n A[收文登记] --> B{是否会签}\n B -- 是 --> C[会签]";
        let dir = tempfile::tempdir().unwrap();
        let png = cache_at(
            dir.path(),
            source,
            Style::Official,
            DiagramTheme::Auto,
            Format::Png,
        )
        .unwrap();
        let pdf = cache_at(
            dir.path(),
            source,
            Style::Official,
            DiagramTheme::Auto,
            Format::Pdf,
        )
        .unwrap();
        assert_eq!(
            &fs::read(dir.path().join(png)).unwrap()[..8],
            b"\x89PNG\r\n\x1a\n"
        );
        assert!(fs::read(dir.path().join(pdf)).unwrap().starts_with(b"%PDF"));
    }

    /// 每种图都能渲出 PNG 与 PDF；桑基与雷达的引擎写死色板要已被换成分类色。
    #[test]
    fn every_kind_renders_both_formats() {
        let dir = tempfile::tempdir().unwrap();
        for (kind, sample) in KIND_SAMPLES {
            for format in [Format::Png, Format::Pdf] {
                let path = cache_at(
                    dir.path(),
                    sample,
                    Style::Official,
                    DiagramTheme::Auto,
                    format,
                )
                .unwrap_or_else(|error| panic!("{kind} {format:?} 渲染失败：{error:#}"));
                let bytes = fs::read(dir.path().join(path)).unwrap();
                match format {
                    Format::Png => assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n"),
                    Format::Pdf => assert!(bytes.starts_with(b"%PDF")),
                }
            }
        }
        // 藏青下桑基节点不再出现引擎自带的红粉色。
        let svg = render_mmdr(KIND_SAMPLES[4].1, Style::Official, DiagramTheme::Navy).unwrap();
        assert!(svg.contains("#1F3A5F"));
        assert!(!svg.contains("#e15759"));
        assert!(!svg.contains("#ff9da7"));
    }

    /// 设了 `GONGWEN_MERMAID_PREVIEW` 就把每种图、每套配色、两种文种的样图写进那个
    /// 目录，调配色时对着看。
    #[test]
    fn every_theme_renders_sample() {
        let dir = tempfile::tempdir().unwrap();
        let preview = std::env::var_os("GONGWEN_MERMAID_PREVIEW").map(std::path::PathBuf::from);
        for theme in DiagramTheme::ALL {
            for style in [Style::Official, Style::Research] {
                for (kind, sample) in KIND_SAMPLES {
                    let image = cache_at(dir.path(), sample, style, theme, Format::Png).unwrap();
                    if let Some(preview) = &preview {
                        fs::create_dir_all(preview).unwrap();
                        fs::copy(
                            dir.path().join(&image),
                            preview.join(format!("{kind}-{style:?}-{theme:?}.png")),
                        )
                        .unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn theme_and_style_change_the_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = "flowchart LR\n A --> B";
        let ink = cache_at(
            dir.path(),
            source,
            Style::Research,
            DiagramTheme::Ink,
            Format::Png,
        );
        let navy = cache_at(
            dir.path(),
            source,
            Style::Research,
            DiagramTheme::Navy,
            Format::Png,
        );
        let auto = cache_at(
            dir.path(),
            source,
            Style::Research,
            DiagramTheme::Auto,
            Format::Png,
        );
        assert_ne!(ink.unwrap(), navy.as_ref().unwrap().clone());
        // 「按文种自动」在研究报告里就是藏青，共用同一张缓存。
        assert_eq!(auto.unwrap(), navy.unwrap());
    }

    #[test]
    fn check_rejects_overrides_but_keeps_arrow_syntax() {
        assert!(check("%% 注释\nflowchart LR\n A <--> B\n C <-.-> D").is_ok());
        assert!(check("graph TD;\n A --> B").is_ok());
        assert!(check("sequenceDiagram\n A->>B: 你好").is_ok());
        assert!(check("gantt\n A :a1, 2026-10-01, 3d").is_ok());
        assert!(check("pie title 构成\n \"甲\" : 1").is_ok());
        assert!(check("sankey-beta\n 甲,乙,1").is_ok());
        assert!(check("timeline\n section 一期\n 任务").is_ok());
        assert!(check("radar-beta\n axis a, b\n curve x {1, 2}").is_ok());
        // 未接入的图种给出清单式报错。
        assert!(check("stateDiagram-v2\n [*] --> 甲").is_err());
        assert!(check("flowchart LR\n A[甲<br>乙] --> B").is_err());
        assert!(check("flowchart LR\n A --> B\n click A \"https://example.com\"").is_err());
        assert!(check("%%{init: {'theme':'dark'}}%%\nflowchart LR\n A --> B").is_err());
        // 节点文字里出现 click 不算点击指令。
        assert!(check("flowchart LR\n A[click 按钮] --> B").is_ok());
    }

    #[test]
    fn official_canvas_is_text_width_and_keeps_font_size() {
        let small = placement(Style::Official, 300.0, 120.0);
        assert!((small.width - OFFICIAL_TEXT_WIDTH_PT).abs() < 1e-6);
        assert!((small.scale - 12.0 / LAYOUT_FONT_PX).abs() < 1e-9);
        // 宽图缩到版心宽，长图缩到限高。
        let wide = placement(Style::Official, 2000.0, 120.0);
        assert!((wide.width - 2000.0 * wide.scale).abs() < 1e-6);
        let tall = placement(Style::Official, 200.0, 3000.0);
        assert!(tall.height <= OFFICIAL_MAX_HEIGHT_RATIO * OFFICIAL_TEXT_HEIGHT_PT + 1e-6);
    }

    #[test]
    fn research_canvas_lands_on_its_own_width_step() {
        let natural = 9.0 / LAYOUT_FONT_PX;
        for (width, height) in [
            (160.0, 90.0),
            (300.0, 260.0),
            (420.0, 420.0),
            (280.0, 950.0),
            (1400.0, 200.0),
            (300.0, 2400.0),
        ] {
            let placed = placement(Style::Research, width, height);
            // mdx 给这块画布的宽度就是画布本身：TeX 与 Word 都不会再缩放。
            let fraction = figure_size::width_fraction(FigureSource::Vector {
                width: placed.width,
                height: placed.height,
            });
            assert!(
                (fraction * RESEARCH_TEXT_WIDTH_PT - placed.width).abs() < 0.01,
                "{width}×{height} 落在 {fraction}"
            );
            assert!(width * placed.scale <= placed.width + 1e-6);
            assert!(placed.scale <= natural + 1e-9);
        }
        // 不高不宽的图保持原样字号。
        assert!((placement(Style::Research, 300.0, 260.0).scale - natural).abs() < 1e-9);
    }

    #[test]
    fn materialize_keeps_caption_and_anchor_for_research_export() {
        let dir = tempfile::tempdir().unwrap();
        let markdown = "前文。\n\n```mermaid\nflowchart LR\n A[收文] --> B[办理]\n```\n图：办理流程 {#fig:flow}\n\n后文。\n";
        let rendered = materialize_at(
            dir.path(),
            markdown,
            Style::Research,
            DiagramTheme::Auto,
            Format::Pdf,
        )
        .unwrap();
        assert!(rendered.contains("![办理流程](mermaid-cache/"));
        assert!(rendered.contains(".pdf){#fig:flow}"));
        assert!(rendered.contains("前文。\n\n"));
        assert!(rendered.ends_with("\n后文。\n"));
        assert!(!rendered.contains("```mermaid"));
        let official = materialize_at(
            dir.path(),
            markdown,
            Style::Official,
            DiagramTheme::Auto,
            Format::Png,
        )
        .unwrap();
        assert!(official.contains("![](mermaid-cache/"));
        assert!(official.contains(".png)\n\n<!-- [居中] -->\n图：办理流程\n\n"));
    }

    #[test]
    fn research_converter_embeds_native_diagram_in_tex_and_docx() {
        use mdx::{ConvertRequest, DocumentStyle, OutputFormat};

        let dir = tempfile::tempdir().unwrap();
        let body = "# 流程研究\n\n<!-- [正文] -->\n\n## 办理环节\n\n```mermaid\nflowchart LR\n A[收文] --> B[办理]\n```\n图：办理流程 {#fig:flow}\n";
        for (format, output_format, extension) in [
            (Format::Pdf, OutputFormat::Tex, "tex"),
            (Format::Png, OutputFormat::Docx, "docx"),
        ] {
            let rendered = materialize_at(
                dir.path(),
                body,
                Style::Research,
                DiagramTheme::Auto,
                format,
            )
            .unwrap();
            let document =
                format!("---\n文件类型: 研究报告\n文件名称: 流程研究\n---\n\n{rendered}");
            let source = dir.path().join("research.md");
            fs::write(&source, document).unwrap();
            let output = dir.path().join(format!("report.{extension}"));
            mdx::convert(ConvertRequest {
                input: source,
                output: Some(output.clone()),
                format: output_format,
                style: DocumentStyle::Research,
                template: None,
                compile_pdf: false,
            })
            .unwrap();
            assert!(output.is_file());
            if extension == "tex" {
                let chapter = fs::read_to_string(dir.path().join("data/chapter01.tex")).unwrap();
                assert!(chapter.contains("figures/"));
            } else {
                let file = fs::File::open(output).unwrap();
                let archive = zip::ZipArchive::new(file).unwrap();
                assert!(
                    archive
                        .file_names()
                        .any(|name| name.starts_with("word/media/"))
                );
            }
        }
    }
}
