//! Mermaid 图表的本机渲染与导出缓存。源码始终保留在 Markdown 中。
//!
//! 一张图的来路：引擎解析排版出 SVG（配色取自 [`DiagramTheme`]，字体随文种）→
//! 按实际字号把图摆到版心宽的画布上 → 用随包字体转 PNG（预览、Word）或 PDF（TeX）。
//! 各导出器照旧把图片铺满画布，图内文字因此总是设定的字号：小图不会被放大成大字，
//! 超出版心的图缩放到可用区域并显示警告，提醒字号可能低于下限。
//!
//! 全部图种都由 merman（纯 Rust，与 Zed 的 Markdown 预览同一引擎，不引浏览器）
//! 解析排版：flowchart / graph（泳道图用 subgraph 分组表达）、sequenceDiagram、
//! gantt、pie、timeline。配色经同一套主题角色、分类色与作用域 CSS 下发，各图种的
//! 线宽、字体、底色一致。桑基图、雷达图经实测版式不成熟，暂不支持，check 会给出提示。

use crate::models::DiagramTheme;
use anyhow::{Context, Result, anyhow, bail};
use mdx::figure_size;
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
const STYLE_VERSION: &str = "gongwen-mermaid-v10/merman-0.7";

/// 排版用的字号（px）。按 18px 测量中文，再换算到纸面字号；
/// 原生时间线和饼图的固定几何留白也随之收紧，文字不缩小。
const LAYOUT_FONT_PX: f64 = 18.0;
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

    /// 图内字号（pt）：公文小四，比三号正文低两档；研究报告五号，刻度和连线标签也不低于此字号。
    fn font_pt(self) -> f64 {
        match self {
            Self::Official => 12.0,
            Self::Research => 10.5,
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
    /// 分类色：目前供饼图扇区循环取色（其余图种尚未开放）。一律浅中调：亮色靠
    /// 色相区分、深色只做锚点，黑白打印下灰阶仍可辨；红色不用。
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
                "#F0F0F0", "#DDDDDD", "#C8C8C8", "#B2B2B2", "#9C9C9C", "#868686", "#6E6E6E",
                "#565656",
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
                "#F4F4F4", "#E4E4E4", "#D2D2D2", "#C0C0C0", "#ACACAC", "#969696", "#7E7E7E",
                "#666666",
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
                "#AFC8E3", "#E2C184", "#9CC6B2", "#C8B48F", "#7E9CC0", "#EAD9AC", "#6FA08C",
                "#B7A273",
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
                "#A9D2C2", "#E2C184", "#8FBFAE", "#C8B48F", "#7CB3A0", "#EAD9AC", "#5E9783",
                "#B7A273",
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
    // 甘特图按给定宽度铺开（默认 1184 px），取版心宽换算回排版 px，落纸时不再缩小。
    let text_width = match style {
        Style::Official => OFFICIAL_TEXT_WIDTH_PT,
        Style::Research => RESEARCH_TEXT_WIDTH_PT,
    } / scale;
    let Palette {
        fill,
        stroke,
        text,
        line: line_color,
        decision,
        cluster_fill,
        cluster_stroke,
        category,
    } = palette;
    // 标题略大，刻度与正文同号，避免中文日期和分支文字低于字号下限。
    let title_px = LAYOUT_FONT_PX + 2.0;
    let tick_px = LAYOUT_FONT_PX;
    // merman 按 SVG 根元素的 id 给这些规则加作用域，排在 Mermaid 自带样式之后，
    // 同权重下后写的生效。框用节点线宽与主色，线用连线线宽与线色，各图种一个样。
    // 甘特的「今天」竖线随渲染日期漂移，纸面上没有意义，缓存也会过期，一律不画。
    let css = format!(
        ".node rect,.node circle,.node ellipse,.node polygon,.node path\
         {{fill:{fill};stroke:{stroke};stroke-width:{node:.2}px;}}\
         .node polygon{{fill:{decision};}}\
         .flowchart-link,.edgePath .path{{stroke:{line_color};stroke-width:{line:.2}px;}}\
         .marker,.marker.cross{{fill:{line_color};stroke:{line_color};}}\
         .arrowheadPath{{fill:{line_color};}}\
         marker path{{fill:{line_color};}}\
         .edgeLabel rect{{opacity:1;fill:#FFFFFF;}}\
         .cluster rect{{fill:{cluster_fill};stroke:{cluster_stroke};\
         stroke-width:{line:.2}px;stroke-dasharray:6 3;}}\
         text{{fill:{text};font-family:{family};font-size:{LAYOUT_FONT_PX}px!important;}}\
         .cluster-label text,.cluster text{{fill:{stroke};}}\
         .pieTitleText,.titleText,&>text{{font-size:{title_px}px!important;font-weight:normal;fill:{text};}}\
         .actor,.note,.labelBox,.activation0,.activation1,.activation2\
         {{stroke-width:{node:.2}px;}}\
         .actor-line,.messageLine0,.messageLine1{{stroke-width:{line:.2}px;}}\
         .loopLine{{stroke:{cluster_stroke};stroke-width:{line:.2}px;stroke-dasharray:6 3;}}\
         .task{{stroke-width:{node:.2}px;}}\
         .today{{display:none;}}\
         .grid .tick line{{stroke:{cluster_stroke};stroke-width:{half:.2}px;opacity:1;}}\
         .grid .tick text{{fill:{text};font-size:{tick_px}px!important;}}\
         .timeline-node .node-bkg{{fill:{decision};stroke:{stroke};stroke-width:{node:.2}px;}}\
         .taskWrapper .timeline-node .node-bkg{{fill:{fill};}}\
         .eventWrapper .timeline-node .node-bkg{{fill:#FFFFFF;}}\
         .timeline-node line{{stroke:none;}}\
         .timeline-node text{{fill:{text};}}\
         .lineWrapper line{{stroke:{line_color};stroke-width:{line:.2}px;}}",
        half = line / 2.0,
        family = style.font_family(),
    );
    let theme = HostThemeProfile::builder()
        .font_family(style.font_family())
        .font_size(format!("{LAYOUT_FONT_PX}px"))
        .roles(HostThemeRoles {
            canvas: Some("#FFFFFF".into()),
            surface: Some(fill.into()),
            // 甘特里已完成的任务退成浅底，进行中的任务与判断节点同一个强调色。
            surface_alt: Some(cluster_fill.into()),
            surface_muted: Some(decision.into()),
            text: Some(text.into()),
            border: Some(stroke.into()),
            line: Some(line_color.into()),
            edge_label_background: Some("#FFFFFF".into()),
            cluster_background: Some(cluster_fill.into()),
            cluster_border: Some(cluster_stroke.into()),
            note_background: Some(decision.into()),
            note_border: Some(stroke.into()),
            actor_background: Some(fill.into()),
            activation_background: Some(fill.into()),
            ..HostThemeRoles::default()
        })
        // 饼图扇区、时间线分区按分类色循环。
        .series_palette(category.iter().copied())
        // 甘特的关键任务默认填红，换成强调色；饼图不叠透明度，打印不发灰。
        .theme_variable("critBkgColor", decision)
        .theme_variable("critBorderColor", stroke)
        .theme_variable("pieOpacity", "1")
        .theme_variable("pieStrokeWidth", format!("{node:.2}px"))
        .theme_variable("pieOuterStrokeWidth", format!("{node:.2}px"))
        .theme_variable("pieTitleTextSize", format!("{title_px}px"))
        .theme_variable("pieSectionTextSize", format!("{LAYOUT_FONT_PX}px"))
        .theme_variable("pieLegendTextSize", format!("{LAYOUT_FONT_PX}px"))
        .output(HostThemeOutput {
            scoped_css: Some(css),
            ..HostThemeOutput::resvg_safe_editor()
        })
        // 比 Mermaid 默认的 50 / 50 / 15 收紧：版心窄，图要紧凑。连线用直线段，
        // 比默认的样条曲线规整；子图标题下多留一点，进来的箭头
        // 不压字。
        .site_config("fontSize", LAYOUT_FONT_PX)
        .site_config("fontFamily", style.font_family())
        .site_config("htmlLabels", false)
        .site_config(
            "flowchart",
            serde_json::json!({
                "htmlLabels": false,
                "curve": "linear",
                "nodeSpacing": 28,
                "rankSpacing": 32,
                "padding": 12,
                "diagramPadding": 10,
                "wrappingWidth": 216,
                "subGraphTitleMargin": { "top": 6, "bottom": 14 },
            }),
        )
        // 甘特按版心宽铺开，任务与分段字号同正文；刻度只写月-日，版心宽里排得下。
        .site_config(
            "gantt",
            serde_json::json!({
                "useWidth": text_width - 24.0,
                "fontSize": LAYOUT_FONT_PX,
                "sectionFontSize": LAYOUT_FONT_PX,
                "barHeight": 32,
                "barGap": 10,
                "topPadding": 48,
                "leftPadding": 132,
                "rightPadding": 64,
                "axisFormat": "%m-%d",
            }),
        )
        // 序列图的角色、消息、备注共用中文字号，角色高度给汉字上下留白。
        .site_config(
            "sequence",
            serde_json::json!({
                "actorMargin": 36, "width": 140, "height": 48,
                "messageMargin": 32, "noteMargin": 12,
                "mirrorActors": false,
            }),
        )
        // 图例放在下方，避免固定直径的饼图加右侧图例后挤出版心。
        .site_config("pie", serde_json::json!({ "legendPosition": "bottom" }))
        // 时间线左侧默认留 150 px 空白，收到与标题对齐。
        .site_config("timeline", serde_json::json!({ "leftMargin": 40 }))
        .build();
    HeadlessRenderer::new()
        .with_host_theme(&theme)
        .with_text_measurer(Arc::new(FontMeasurer {
            inner: VendoredFontMetricsTextMeasurer::default(),
        }))
        .with_strict_parsing()
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

/// 已接入的图头。merman 认得的图种更多，这里只放行调过版式的几种。
fn supported(source: &str) -> bool {
    matches!(
        header(source),
        "flowchart" | "graph" | "sequenceDiagram" | "gantt" | "pie" | "timeline"
    )
}

/// 不让单张图改版式、引外部资源；只放行已接入引擎的图种。
fn check(source: &str) -> Result<()> {
    if !supported(source) {
        bail!(
            "暂不支持的 Mermaid 图种「{}」：目前支持 flowchart/graph（含泳道分组）、\
             sequenceDiagram、gantt、pie、timeline；桑基图与雷达图暂不成熟，后续开放",
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

fn placement(style: Style, width: f64, height: f64) -> Result<Placement> {
    let natural = style.font_pt() / LAYOUT_FONT_PX;
    let (text_width, max_height) = match style {
        Style::Official => (
            OFFICIAL_TEXT_WIDTH_PT,
            OFFICIAL_MAX_HEIGHT_RATIO * OFFICIAL_TEXT_HEIGHT_PT,
        ),
        Style::Research => (
            RESEARCH_TEXT_WIDTH_PT,
            figure_size::MAX_HEIGHT_RATIO * RESEARCH_TEXT_HEIGHT_PT,
        ),
    };
    let scale = natural.min(text_width / width).min(max_height / height);
    Ok(Placement {
        width: text_width,
        height: height * scale,
        scale,
    })
}

/// 警告跟随图片缓存保存，预览与导出均可提示，重新打开应用后仍可读取。
fn size_warning(style: Style, placed: Placement) -> Option<String> {
    let font = placed.scale * LAYOUT_FONT_PX;
    (font < style.font_pt() - 1e-6).then(|| {
        format!(
            "图表过大，已缩小以完整显示；图中文字约为 {font:.1} pt，低于本稿要求的 {:.1} pt。请缩短节点文字、改为纵向排列或拆成多张图。",
            style.font_pt()
        )
    })
}

pub(crate) fn cached_warning(path: &str) -> Option<String> {
    warning_at(&crate::storage::config_dir().ok()?, path)
}

fn warning_at(base: &Path, path: &str) -> Option<String> {
    fs::read_to_string(base.join(path).with_extension("warning.txt"))
        .ok()
        .filter(|text| !text.is_empty())
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

/// 甘特自动日期刻度按西文小字估算间距；保留网格，只疏排会重叠的日期标签。
/// 末端刻度预留空间，不能为容纳日期再缩小字体。
fn space_gantt_ticks(svg: &str, style: Style) -> String {
    static TICKS: OnceLock<regex::Regex> = OnceLock::new();
    let ticks = TICKS.get_or_init(|| {
        regex::Regex::new(
            r#"<g class="tick" opacity="1" transform="translate\(([-0-9.]+),0\)">.*?<text[^>]*>([^<]*)</text></g>"#,
        ).unwrap()
    });
    let measurer = FontMeasurer {
        inner: VendoredFontMetricsTextMeasurer::default(),
    };
    let text_style = TextStyle {
        font_family: Some(style.font_family().into()),
        font_size: LAYOUT_FONT_PX,
        font_weight: None,
    };
    let labels: Vec<_> = ticks.captures_iter(svg).collect();
    if labels.is_empty() {
        return svg.to_string();
    }
    let bounds = |label: &regex::Captures<'_>| {
        let x: f64 = label[1].parse().unwrap_or(0.0);
        let width = measurer.width(&label[2], &text_style);
        (x - width / 2.0, x + width / 2.0)
    };
    let mut last_left = 0.0;
    let gap = LAYOUT_FONT_PX * 0.6;
    let mut previous_right = f64::NEG_INFINITY;
    let mut result = String::with_capacity(svg.len());
    let mut end = 0;
    let mut run_end = 0;
    for (index, label) in labels.iter().enumerate() {
        if index == run_end {
            run_end = (index + 1..labels.len())
                .find(|next| bounds(&labels[*next]).0 < bounds(&labels[*next - 1]).0)
                .unwrap_or(labels.len());
            last_left = bounds(&labels[run_end - 1]).0;
            previous_right = f64::NEG_INFINITY;
        }
        let matched = label.get(0).unwrap();
        result.push_str(&svg[end..matched.start()]);
        let (left, right) = bounds(label);
        let keep =
            left >= previous_right + gap && (index + 1 == run_end || right + gap <= last_left);
        if keep {
            result.push_str(matched.as_str());
            previous_right = right;
        } else {
            let group = matched.as_str();
            let start = group.find("<text").unwrap();
            let stop = group.find("</text>").unwrap() + "</text>".len();
            result.push_str(&group[..start]);
            result.push_str(&group[stop..]);
        }
        end = matched.end();
    }
    result.push_str(&svg[end..]);
    result
}

/// 复用引擎生成的扇区和文字，单独缩小几何，图例横排并在超宽时换行。
fn compact_pie(svg: &str, style: Style) -> Result<String> {
    let regex = |pattern| regex::Regex::new(pattern).unwrap();
    let paths = regex(r#"<path[^>]*class="pieCircle"[^>]*/>"#);
    let slices = regex(
        r#"<text transform="translate\(([-0-9.]+),([-0-9.]+)\)" class="slice"[^>]*>(.*?)</text>"#,
    );
    let legends = regex(r#"<g class="legend"[^>]*>(<rect[^>]*/>)<text[^>]*>(.*?)</text></g>"#);
    let title = regex(r#"<text[^>]*class="pieTitleText"[^>]*>(.*?)</text>"#);
    let styles = regex(r"(?s)<style>.*?</style>");
    let measurer = FontMeasurer {
        inner: VendoredFontMetricsTextMeasurer::default(),
    };
    let text_style = TextStyle {
        font_family: Some(style.font_family().into()),
        font_size: LAYOUT_FONT_PX,
        font_weight: None,
    };
    let items: Vec<_> = legends.captures_iter(svg).collect();
    if items.is_empty() {
        return Ok(svg.to_string());
    }
    let max_width = 420.0;
    let mut rows: Vec<Vec<(String, String, f64)>> = vec![Vec::new()];
    let mut row_width = 0.0;
    for item in items {
        let width = 26.0 + measurer.width(&item[2], &text_style) + 20.0;
        if row_width + width > max_width && !rows.last().unwrap().is_empty() {
            rows.push(Vec::new());
            row_width = 0.0;
        }
        rows.last_mut()
            .unwrap()
            .push((item[1].to_string(), item[2].to_string(), width));
        row_width += width;
    }
    let width = rows
        .iter()
        .map(|row| row.iter().map(|item| item.2).sum::<f64>())
        .fold(260.0, f64::max)
        + 24.0;
    let factor = 0.62;
    let radius = 185.0 * factor;
    let title_text = title.captures(svg).map(|capture| capture[1].to_string());
    let top = if title_text.is_some() { 40.0 } else { 12.0 };
    let center_y = top + radius;
    let legend_y = center_y + radius + 22.0;
    let height = legend_y + rows.len() as f64 * 30.0 + 8.0;
    let start = svg.find('>').context("饼图 SVG 缺少根元素")?;
    let mut root = svg[..start].to_string();
    for name in ["width", "height", "viewBox", "style"] {
        root = strip_attribute(&root, name);
    }
    let mut result =
        format!("{root} width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">");
    for css in styles.find_iter(svg) {
        result.push_str(css.as_str());
    }
    if let Some(title) = title_text {
        result.push_str(&format!(
            "<text class=\"pieTitleText\" x=\"{}\" y=\"24\" text-anchor=\"middle\">{title}</text>",
            width / 2.0
        ));
    }
    result.push_str(&format!(
        "<g transform=\"translate({}, {center_y})\"><g transform=\"scale({factor})\">",
        width / 2.0
    ));
    for path in paths.find_iter(svg) {
        result.push_str(path.as_str());
    }
    result.push_str("</g>");
    for label in slices.captures_iter(svg) {
        let x = label[1].parse::<f64>().context("饼图标签横坐标无效")? * factor;
        let y = label[2].parse::<f64>().context("饼图标签纵坐标无效")? * factor;
        result.push_str(&format!(
            "<text class=\"slice\" x=\"{x}\" y=\"{y}\" text-anchor=\"middle\">{}</text>",
            &label[3]
        ));
    }
    result.push_str("</g>");
    for (index, row) in rows.iter().enumerate() {
        let mut x = (width - row.iter().map(|item| item.2).sum::<f64>() + 20.0) / 2.0;
        let y = legend_y + index as f64 * 30.0;
        for (rect, label, item_width) in row {
            result.push_str(&format!("<g class=\"legend\" transform=\"translate({x},{y})\">{rect}<text x=\"26\" y=\"15\">{label}</text></g>"));
            x += item_width;
        }
    }
    result.push_str("</svg>");
    Ok(result)
}

/// 压缩时间线的纵向几何，文字反向补偿保持纸面字号；按可见边界收掉四周空白。
fn compact_timeline(svg: &str) -> Result<String> {
    let title = regex::Regex::new(
        r#"<text x="([^"]+)" font-size="4ex" font-weight="bold" y="([^"]+)">(.*?)</text>"#,
    )
    .unwrap();
    let svg = title.replace_all(svg, |capture: &regex::Captures<'_>| {
        format!(
            "<text class=\"titleText\" x=\"{}\" y=\"{}\">{}</text>",
            &capture[1], &capture[2], &capture[3]
        )
    });
    let start = svg.find('>').context("时间线 SVG 缺少根元素")?;
    let end = svg.rfind("</svg>").context("时间线 SVG 未闭合")?;
    let text = regex::Regex::new(r"<text ").unwrap();
    let content = text.replace_all(&svg[start + 1..end], "<text transform=\"scale(1,1.25)\" ");
    let compressed = format!(
        "{}><g transform=\"scale(1,0.8)\">{content}</g></svg>",
        &svg[..start]
    );
    let view =
        regex::Regex::new(r#"viewBox="([-0-9.]+) ([-0-9.]+) ([-0-9.]+) ([-0-9.]+)""#).unwrap();
    let viewport = view.captures(&compressed).context("时间线缺少 viewBox")?;
    let viewport_width = viewport[3].parse::<f64>()?;
    let viewport_height = viewport[4].parse::<f64>()?;
    // 百分比根尺寸会引入默认视口的等比居中留白，测量前明确指定 viewBox 尺寸。
    let root_end = compressed.find('>').unwrap();
    let mut root = compressed[..root_end].to_string();
    for name in ["width", "height", "style"] {
        root = strip_attribute(&root, name);
    }
    let compressed = format!(
        "{root} width=\"{viewport_width}\" height=\"{viewport_height}\"{}",
        &compressed[root_end..]
    );
    let options = usvg::Options {
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&compressed, &options).context("时间线边界测量失败")?;
    // 零面积辅助线会污染几何边界，用透明画布的实际绘制外缘定尺寸。
    // 仅测量边界，PDF 中的图形和文字仍保留矢量。
    let zoom = (1200.0 / tree.size().width())
        .min(1200.0 / tree.size().height())
        .min(2.0);
    let mut pixels = tiny_skia::Pixmap::new(
        (tree.size().width() * zoom).ceil() as u32,
        (tree.size().height() * zoom).ceil() as u32,
    )
    .context("时间线测量画布无效")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(zoom, zoom),
        &mut pixels.as_mut(),
    );
    let (mut left_px, mut top_px, mut right_px, mut bottom_px) =
        (pixels.width(), pixels.height(), 0, 0);
    for (index, pixel) in pixels.pixels().iter().enumerate() {
        if pixel.alpha() > 0 {
            let x = index as u32 % pixels.width();
            let y = index as u32 / pixels.width();
            left_px = left_px.min(x);
            top_px = top_px.min(y);
            right_px = right_px.max(x);
            bottom_px = bottom_px.max(y);
        }
    }
    let bounds = usvg::Rect::from_ltrb(
        left_px as f32 / zoom,
        top_px as f32 / zoom,
        (right_px + 1) as f32 / zoom,
        (bottom_px + 1) as f32 / zoom,
    )
    .context("时间线没有可见内容")?;
    let view =
        regex::Regex::new(r#"viewBox="([-0-9.]+) ([-0-9.]+) ([-0-9.]+) ([-0-9.]+)""#).unwrap();
    let original = view.captures(&compressed).context("时间线缺少 viewBox")?;
    let x = original[1].parse::<f64>()?;
    let y = original[2].parse::<f64>()?;
    let w = original[3].parse::<f64>()?;
    let h = original[4].parse::<f64>()?;
    let sx = w / f64::from(tree.size().width());
    let sy = h / f64::from(tree.size().height());
    let left = x + f64::from(bounds.left()) * sx - 12.0;
    let top = y + f64::from(bounds.top()) * sy - 12.0;
    let width = f64::from(bounds.width()) * sx + 24.0;
    let height = f64::from(bounds.height()) * sy + 24.0;
    let centered_title =
        regex::Regex::new(r#"<text transform="scale\(1,1.25\)" class="titleText" x="[^"]+""#)
            .unwrap();
    let compressed = centered_title.replace(
        &compressed,
        format!(
            "<text transform=\"scale(1,1.25)\" class=\"titleText\" text-anchor=\"middle\" x=\"{}\"",
            left + width / 2.0
        ),
    );
    Ok(view
        .replace(
            &compressed,
            format!("viewBox=\"{left} {top} {width} {height}\""),
        )
        .into_owned())
}

/// 只翻译引擎生成的分组角标；消息、角色与用户写的条件说明保持原文。
fn chinese_sequence_labels(svg: &str) -> String {
    static LABELS: OnceLock<regex::Regex> = OnceLock::new();
    let labels = LABELS.get_or_init(|| {
        regex::Regex::new(r#"(<text\b[^>]*class="labelText"[^>]*>)(loop|alt)(</text>)"#)
            .expect("序列图角标匹配式有效")
    });
    labels
        .replace_all(svg, |capture: &regex::Captures<'_>| {
            let label = match &capture[2] {
                "loop" => "循环",
                "alt" => "条件",
                _ => unreachable!(),
            };
            format!("{}{label}{}", &capture[1], &capture[3])
        })
        .into_owned()
}

/// 排版修整由预览和两种导出格式共用，不改变稿件中的图表源码。
fn layout_svg(source: &str, style: Style, theme: DiagramTheme) -> Result<String> {
    check(source)?;
    let svg = renderer(style, theme)
        .render_svg_sync(source)
        .map_err(|error| anyhow!("Mermaid 解析或排版失败：{error}"))?
        .context("Mermaid 围栏里没有图形")?;
    match header(source) {
        "sequenceDiagram" => Ok(chinese_sequence_labels(&svg)),
        "gantt" => Ok(space_gantt_ticks(&svg, style)),
        "pie" => compact_pie(&svg, style),
        "timeline" => compact_timeline(&svg),
        _ => Ok(svg),
    }
}

fn render(
    source: &str,
    style: Style,
    theme: DiagramTheme,
    format: Format,
) -> Result<(Vec<u8>, Option<String>)> {
    let svg = layout_svg(source, style, theme)?;
    let (width, height) = view_box(&svg).context("Mermaid 输出缺少 viewBox")?;
    let placement = placement(style, width, height)?;
    let bytes = match format {
        Format::Png => to_png(&compose(&svg, placement, Some("#FFFFFF"))?),
        Format::Pdf => to_pdf(&compose(&svg, placement, None)?),
    }?;
    Ok((bytes, size_warning(style, placement)))
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
        "gongwen-mermaid-canvas-{}.{}",
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
    let (bytes, warning) = match render(source, style, theme, format) {
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
    fs::write(
        path.with_extension("warning.txt"),
        warning.unwrap_or_default(),
    )?;
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
        if let Some(warning) = warning_at(base, &path) {
            out.push_str(&format!("\n图表警告：{warning}\n\n"));
        }
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

    /// 用实际 Markdown 测试稿批量验证图例，输出逐图结果和样图供人工复核。
    #[test]
    #[ignore = "需要设置 GONGWEN_MERMAID_TEST_FILE 与 GONGWEN_MERMAID_PREVIEW"]
    fn comprehensive_markdown_fixture_renders_in_all_styles() {
        let file = std::env::var_os("GONGWEN_MERMAID_TEST_FILE").expect("缺少测试稿路径");
        let output = std::path::PathBuf::from(
            std::env::var_os("GONGWEN_MERMAID_PREVIEW").expect("缺少样图目录"),
        );
        fs::create_dir_all(&output).unwrap();
        let markdown = fs::read_to_string(file).unwrap();
        let blocks = crate::export::parse_markdown(&markdown);
        let diagrams: Vec<_> = blocks
            .iter()
            .filter_map(|block| {
                if let crate::export::MarkdownBlock::Diagram { source, caption } = block {
                    Some((source, caption))
                } else {
                    None
                }
            })
            .collect();
        assert!(!diagrams.is_empty(), "测试稿没有 Mermaid 图例");
        let cache = tempfile::tempdir().unwrap();
        let mut records = Vec::new();
        let mut failures = Vec::new();
        for (index, (source, caption)) in diagrams.iter().enumerate() {
            for style in [Style::Official, Style::Research] {
                for theme in DiagramTheme::ALL {
                    for format in [Format::Png, Format::Pdf] {
                        let result = cache_at(cache.path(), source, style, theme, format);
                        let error = result.as_ref().err().map(|error| format!("{error:#}"));
                        if let Ok(relative) = result {
                            let name = format!(
                                "case{:02}-{style:?}-{theme:?}.{}",
                                index + 1,
                                format.extension()
                            );
                            fs::copy(cache.path().join(relative), output.join(name)).unwrap();
                        }
                        if let Some(error) = &error {
                            failures.push(format!(
                                "案例{:02} {caption} {style:?} {theme:?} {format:?}: {error}",
                                index + 1
                            ));
                        }
                        records.push(serde_json::json!({"case": index + 1, "caption": caption, "style": format!("{style:?}"), "theme": format!("{theme:?}"), "format": format.extension(), "error": error}));
                    }
                }
            }
        }
        fs::write(
            output.join("validation.json"),
            serde_json::to_string_pretty(&records).unwrap(),
        )
        .unwrap();
        println!(
            "{} 个图例，共 {} 项渲染验证，失败 {} 项",
            diagrams.len(),
            records.len(),
            failures.len()
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    const SAMPLE: &str = "flowchart TD\n A[收文登记] --> B{是否会签}\n B -- 是 --> C[相关单位会签]\n B -- 否 --> D[审核签发]\n C --> D\n D --> E[归档保存]";

    /// 本期接入的全部图种：一份能代表该图种最小语法的中文样例。
    const KIND_SAMPLES: &[(&str, &str)] = &[
        ("flowchart", SAMPLE),
        (
            "sequence",
            "sequenceDiagram\n  经办人->>处长: 提交请示\n  处长-->>经办人: 退回修改\n  Note over 处长: 研究\n  loop 修改\n    经办人->>处长: 再次报送\n  end",
        ),
        (
            "gantt",
            "gantt\n  title 项目进度\n  dateFormat YYYY-MM-DD\n  section 调研\n  收集资料 :done, a1, 2026-10-01, 10d\n  撰写报告 :active, a2, after a1, 7d\n  section 审定\n  内部评审 :crit, a3, after a2, 5d",
        ),
        (
            "pie",
            "pie title 经费构成\n  \"人员经费\" : 45\n  \"公用经费\" : 30\n  \"项目支出\" : 25",
        ),
        (
            "timeline",
            "timeline\n  title 工作规划\n  section 2026年 第四季度\n    完成调研 : 形成调研报告\n  section 2027年 上半年\n    出台办法 : 印发实施",
        ),
    ];

    /// 字号要按最终 SVG 的变换核对，而非只检查主题配置里的数值。
    ///
    /// CI 上的最小 Debian 容器里 usvg 把 merman 的中文文本节点丢光，只剩路径占位；
    /// Windows 与 macOS 下正常出文字节点，能校验字号。这种环境下没文字可校，跳过
    /// 字号断言并在 stderr 留一条说明。
    #[test]
    fn rendered_text_keeps_the_document_font_floor() {
        fn check_text(group: &usvg::Group, minimum: f64) -> usize {
            let mut count = 0;
            for node in group.children() {
                match node {
                    usvg::Node::Group(group) => count += check_text(group, minimum),
                    usvg::Node::Text(text) => {
                        let scale = f64::from(text.abs_transform().sy);
                        for chunk in text.chunks() {
                            for span in chunk.spans() {
                                let actual = f64::from(span.font_size().get()) * scale;
                                assert!(actual >= minimum - 0.01, "文字只有 {actual} pt");
                                count += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
            count
        }
        // 至少要在任一 (style, kind) 组合下拿到文字节点，否则视作环境无法校验字体，
        // 跳过本测试；Windows / macOS 与装了完整字体的 CI 都应至少有若干组合拿到。
        let any_text = [Style::Official, Style::Research]
            .into_iter()
            .flat_map(|style| {
                KIND_SAMPLES
                    .iter()
                    .map(move |(kind, source)| (style, kind, source))
            })
            .any(|(style, _kind, source)| {
                let svg = layout_svg(source, style, DiagramTheme::Auto).unwrap();
                let (width, height) = view_box(&svg).unwrap();
                let canvas = compose(&svg, placement(style, width, height).unwrap(), None).unwrap();
                let tree = usvg::Tree::from_str(
                    &canvas,
                    &usvg::Options {
                        fontdb: fonts(),
                        ..usvg::Options::default()
                    },
                )
                .unwrap();
                check_text(tree.root(), style.font_pt()) > 0
            });
        if !any_text {
            eprintln!("跳过字号下限校验：当前环境 usvg 拿不到文字节点（CI 最小 Debian 容器已知）");
            return;
        }
        for style in [Style::Official, Style::Research] {
            for (kind, source) in KIND_SAMPLES {
                let svg = layout_svg(source, style, DiagramTheme::Auto).unwrap();
                let (width, height) = view_box(&svg).unwrap();
                let canvas = compose(&svg, placement(style, width, height).unwrap(), None).unwrap();
                let tree = usvg::Tree::from_str(
                    &canvas,
                    &usvg::Options {
                        fontdb: fonts(),
                        ..usvg::Options::default()
                    },
                )
                .unwrap();
                let count = check_text(tree.root(), style.font_pt());
                assert!(count > 0, "{kind} 没有文字");
            }
        }
    }

    #[test]
    fn gantt_date_labels_do_not_overlap_or_clip() {
        fn dates(group: &usvg::Group, boxes: &mut Vec<usvg::Rect>) {
            for node in group.children() {
                match node {
                    usvg::Node::Group(group) => dates(group, boxes),
                    usvg::Node::Text(text)
                        if text.chunks().iter().any(|chunk| {
                            let label = chunk.text();
                            label.chars().count() == 5 && label.starts_with("10")
                        }) =>
                    {
                        boxes.push(text.abs_bounding_box())
                    }
                    _ => {}
                }
            }
        }
        let mut parsed: Vec<(Style, Vec<usvg::Rect>, f32)> = Vec::new();
        for style in [Style::Official, Style::Research] {
            let source = KIND_SAMPLES
                .iter()
                .find(|(kind, _)| *kind == "gantt")
                .unwrap()
                .1;
            let svg = layout_svg(source, style, DiagramTheme::Auto).unwrap();
            let (width, height) = view_box(&svg).unwrap();
            let canvas = compose(&svg, placement(style, width, height).unwrap(), None).unwrap();
            let tree = usvg::Tree::from_str(
                &canvas,
                &usvg::Options {
                    fontdb: fonts(),
                    ..usvg::Options::default()
                },
            )
            .unwrap();
            let mut boxes = Vec::new();
            dates(tree.root(), &mut boxes);
            parsed.push((style, boxes, tree.size().width()));
        }
        // CI 上的最小 Debian 容器里 usvg 把甘特中文日期刻度丢光，没法校重叠；正常
        // 环境（Windows / macOS）至少 Official 或 Research 之一会有日期标签。
        if parsed.iter().all(|(_, boxes, _)| boxes.is_empty()) {
            eprintln!(
                "跳过甘特日期刻度重叠校验：当前环境 usvg 拿不到文字节点（CI 最小 Debian 容器已知）"
            );
            return;
        }
        for (style, mut boxes, width) in parsed {
            assert!(boxes.len() >= 2, "{style:?} 甘特日期标签不足两条");
            boxes.sort_by(|a, b| a.left().total_cmp(&b.left()));
            for pair in boxes.windows(2) {
                assert!(pair[0].right() < pair[1].left(), "日期刻度重叠");
            }
            for rect in boxes {
                assert!(rect.left() >= 0.0 && rect.right() <= width, "日期被裁切");
            }
        }
    }

    #[test]
    fn timeline_has_a_title_and_small_outer_margins() {
        let source = KIND_SAMPLES
            .iter()
            .find(|(kind, _)| *kind == "timeline")
            .unwrap()
            .1;
        let svg = layout_svg(source, Style::Research, DiagramTheme::Auto).unwrap();
        assert!(svg.contains("class=\"titleText\""));
        assert!(svg.contains("工作规划"));
        let (width, height) = view_box(&svg).unwrap();
        let canvas = compose(
            &svg,
            placement(Style::Research, width, height).unwrap(),
            None,
        )
        .unwrap();
        let bytes = to_png(&canvas).unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let ink: Vec<_> = (0..image.height())
            .filter(|y| {
                (0..image.width()).any(|x| {
                    image
                        .get_pixel(x, *y)
                        .0
                        .iter()
                        .any(|channel| *channel < 240)
                })
            })
            .collect();
        assert!(*ink.first().unwrap() <= 40, "时间线上方留白过大");
        assert!(
            image.height() - ink.last().unwrap() <= 40,
            "时间线下方留白过大"
        );
    }

    #[test]
    fn pie_legends_share_one_row_and_chart_is_smaller() {
        let source = KIND_SAMPLES
            .iter()
            .find(|(kind, _)| *kind == "pie")
            .unwrap()
            .1;
        let original = renderer(Style::Research, DiagramTheme::Auto)
            .render_svg_sync(source)
            .unwrap()
            .unwrap();
        let compact = layout_svg(source, Style::Research, DiagramTheme::Auto).unwrap();
        assert!(view_box(&compact).unwrap().1 < view_box(&original).unwrap().1 * 0.8);
        let regex =
            regex::Regex::new(r#"class="legend" transform="translate\([^,]+,([^)]*)\)""#).unwrap();
        let rows: Vec<_> = regex
            .captures_iter(&compact)
            .map(|capture| capture[1].to_string())
            .collect();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row == &rows[0]));
    }

    #[test]
    fn sequence_loop_and_alt_labels_are_chinese_in_all_styles() {
        let source = "sequenceDiagram\n participant A as 经办人\n participant B as 审核人\n loop 复核\n alt 材料齐全\n A->>B: loop\n else 补充材料\n B-->>A: alt\n end\n end";
        let labels = regex::Regex::new(r#"class="labelText"[^>]*>([^<]+)</text>"#).unwrap();
        for style in [Style::Official, Style::Research] {
            for theme in DiagramTheme::ALL {
                let svg = layout_svg(source, style, theme).unwrap();
                let actual: Vec<_> = labels
                    .captures_iter(&svg)
                    .map(|capture| capture[1].to_owned())
                    .collect();
                assert!(actual.contains(&"循环".to_owned()));
                assert!(actual.contains(&"条件".to_owned()));
                assert!(!actual.iter().any(|label| label == "loop" || label == "alt"));
                assert!(svg.contains("复核"));
                assert!(svg.contains("材料齐全"));
                assert!(svg.contains("补充材料"));
                assert!(svg.contains(">loop</"));
                assert!(svg.contains(">alt</"));
                for format in [Format::Png, Format::Pdf] {
                    assert!(!render(source, style, theme, format).unwrap().0.is_empty());
                }
            }
        }
    }

    #[test]
    fn oversized_diagram_still_renders_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let source = "flowchart LR\n A[收文登记办理归档收文登记办理归档] --> B[收文登记办理归档收文登记办理归档] --> C[收文登记办理归档收文登记办理归档]";
        for format in [Format::Png, Format::Pdf] {
            let markdown = format!("前文。\n\n```mermaid\n{source}\n```\n图：超宽测试\n");
            let output = materialize_at(
                dir.path(),
                &markdown,
                Style::Official,
                DiagramTheme::Auto,
                format,
            )
            .unwrap();
            assert!(output.contains("图表警告："));
            assert!(output.contains("低于本稿要求"));
            assert!(output.contains("超宽测试"));
            let path = cache_at(
                dir.path(),
                source,
                Style::Official,
                DiagramTheme::Auto,
                format,
            )
            .unwrap();
            assert!(dir.path().join(&path).is_file());
            assert!(warning_at(dir.path(), &path).is_some());
        }
    }

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

    /// 每种图都能渲出 PNG 与 PDF。
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
    }

    /// 全部图种走同一套主题：饼图扇区取分类色，甘特的关键任务不落 Mermaid 默认的红，
    /// 「今天」竖线不画，序列、时间线与流程图同一支线色。
    #[test]
    fn every_kind_follows_the_palette() {
        let palette = palette(DiagramTheme::Navy, Style::Research);
        let svg = |kind: &str| {
            let sample = KIND_SAMPLES
                .iter()
                .find(|(name, _)| *name == kind)
                .unwrap()
                .1;
            renderer(Style::Research, DiagramTheme::Navy)
                .render_svg_sync(sample)
                .unwrap()
                .unwrap()
        };
        assert!(svg("pie").contains(palette.category[0]));
        let gantt = svg("gantt");
        assert!(!gantt.contains("fill:red"));
        assert!(gantt.contains(".today {display:none;}"));
        for kind in ["sequence", "timeline"] {
            assert!(svg(kind).contains(palette.line), "{kind}");
        }
    }

    /// 桑基图与雷达图版式不成熟，明确拒绝并提示后续开放。
    #[test]
    fn sankey_and_radar_are_not_supported_yet() {
        assert!(check("sankey-beta\n 甲,乙,1").is_err());
        assert!(check("radar-beta\n axis 甲, 乙\n curve 丙 {1, 2}").is_err());
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
                        if *kind == "timeline"
                            && style == Style::Research
                            && theme == DiagramTheme::Auto
                        {
                            fs::write(
                                preview.join("timeline-layout.svg"),
                                layout_svg(sample, style, theme).unwrap(),
                            )
                            .unwrap();
                        }
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
        assert!(check("timeline\n section 一期\n 任务").is_ok());
        // 未接入的图种给出清单式报错。
        assert!(check("stateDiagram-v2\n [*] --> 甲").is_err());
        assert!(check("sankey-beta\n 甲,乙,1").is_err());
        assert!(check("radar-beta\n axis a, b\n curve x {1, 2}").is_err());
        assert!(check("flowchart LR\n A[甲<br>乙] --> B").is_err());
        assert!(check("flowchart LR\n A --> B\n click A \"https://example.com\"").is_err());
        assert!(check("%%{init: {'theme':'dark'}}%%\nflowchart LR\n A --> B").is_err());
        // 节点文字里出现 click 不算点击指令。
        assert!(check("flowchart LR\n A[click 按钮] --> B").is_ok());
    }

    #[test]
    fn official_canvas_is_text_width_and_keeps_font_size() {
        let small = placement(Style::Official, 300.0, 120.0).unwrap();
        assert!((small.width - OFFICIAL_TEXT_WIDTH_PT).abs() < 1e-6);
        assert!((small.scale - 12.0 / LAYOUT_FONT_PX).abs() < 1e-9);
        // 超宽、超高仍显示，并明确提示缩小后的字号。
        assert!(
            size_warning(
                Style::Official,
                placement(Style::Official, 2000.0, 120.0).unwrap()
            )
            .is_some()
        );
        assert!(
            size_warning(
                Style::Official,
                placement(Style::Official, 200.0, 3000.0).unwrap()
            )
            .is_some()
        );
    }

    #[test]
    fn research_canvas_keeps_font_size_without_bottom_padding() {
        let natural = 10.5 / LAYOUT_FONT_PX;
        for (width, height) in [(160.0, 90.0), (300.0, 260.0), (420.0, 420.0)] {
            let placed = placement(Style::Research, width, height).unwrap();
            assert!((placed.width - RESEARCH_TEXT_WIDTH_PT).abs() < 0.01);
            assert!(width * placed.scale <= placed.width + 1e-6);
            assert!((placed.scale - natural).abs() < 1e-9);
            assert!((placed.height - height * natural).abs() < 1e-9);
        }
        assert!(
            size_warning(
                Style::Research,
                placement(Style::Research, 1400.0, 200.0).unwrap()
            )
            .is_some()
        );
        assert!(
            size_warning(
                Style::Research,
                placement(Style::Research, 300.0, 2400.0).unwrap()
            )
            .is_some()
        );
    }

    #[test]
    fn short_horizontal_diagram_has_no_artificial_bottom_space() {
        let source = "flowchart LR\n A[收文] --> B[办理] --> C[归档]";
        let svg = layout_svg(source, Style::Research, DiagramTheme::Auto).unwrap();
        let (width, height) = view_box(&svg).unwrap();
        let placed = placement(Style::Research, width, height).unwrap();
        assert!((placed.height - height * 10.5 / LAYOUT_FONT_PX).abs() < 1e-9);
        let dir = tempfile::tempdir().unwrap();
        for format in [Format::Png, Format::Pdf] {
            let relative = cache_at(
                dir.path(),
                source,
                Style::Research,
                DiagramTheme::Auto,
                format,
            )
            .unwrap();
            let path = dir.path().join(relative);
            let size = mdx::figure_size::probe(&path, None).unwrap();
            assert_eq!(
                mdx::figure_size::width_fraction_for_path(size, Path::new("figures/photo.png")),
                mdx::figure_size::width_fraction(size),
            );
            if let Some(preview) = std::env::var_os("GONGWEN_MERMAID_PREVIEW") {
                let preview = std::path::PathBuf::from(preview);
                fs::create_dir_all(&preview).unwrap();
                fs::copy(
                    &path,
                    preview.join(format!("horizontal-Research.{}", format.extension())),
                )
                .unwrap();
            }
            assert_eq!(mdx::figure_size::width_fraction_for_path(size, &path), 1.0);
        }
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
    fn research_converter_embeds_native_diagram_in_pdf_data_and_docx() {
        use mdx::{ConvertRequest, DocumentStyle, OutputFormat};

        let dir = tempfile::tempdir().unwrap();
        let body = "# 流程研究\n\n<!-- [正文] -->\n\n## 办理环节\n\n见图 {@fig:flow}。\n\n```mermaid\nflowchart LR\n A[收文] --> B[办理]\n```\n图：办理流程 {#fig:flow}\n";
        let source_for = |format: Format| {
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
            source
        };

        // PDF：Typst 排版数据里是一张满宽、带图号的插图。
        let doc = mdx::typst_research::build(&source_for(Format::Pdf)).unwrap();
        let data = serde_json::to_value(&doc).unwrap();
        let figure = data["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["k"] == "figure")
            .expect("图表应排成插图");
        assert_eq!(figure["width"], 1.0, "图表宽度必须保持满版心");
        assert_eq!(figure["number"], "1.1");
        assert_eq!(figure["label"], "fig:flow");

        // Word。
        let output = dir.path().join("report.docx");
        mdx::convert(ConvertRequest {
            input: source_for(Format::Png),
            output: Some(output.clone()),
            format: OutputFormat::Docx,
            style: DocumentStyle::Research,
        })
        .unwrap();
        let file = fs::File::open(output).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert!(
            archive
                .file_names()
                .any(|name| name.starts_with("word/media/"))
        );
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("word/document.xml").unwrap(), &mut xml)
            .unwrap();
        assert!(xml.contains("cx=\"5616000\""), "图表宽度必须保持 156 mm");
    }
}
