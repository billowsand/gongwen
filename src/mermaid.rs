//! Mermaid 图表的本机渲染与导出缓存。源码始终保留在 Markdown 中。
//!
//! 一张图的来路：引擎解析排版出 SVG（配色取自 [`DiagramTheme`]，字体随文种）→
//! 按实际字号把图摆到版心宽的画布上 → 用随包字体转 PNG（预览、Word）或 PDF（TeX）。
//! 各导出器照旧把图片铺满画布，图内文字因此总是设定的字号：小图不会被放大成大字，
//! 宽图、长图才整体缩小到版心以内。
//!
//! 全部图种都由 merman（纯 Rust，与 Zed 的 Markdown 预览同一引擎，不引浏览器）
//! 解析排版：flowchart / graph（泳道图用 subgraph 分组表达）、sequenceDiagram、
//! gantt、pie、timeline。配色经同一套主题角色、分类色与作用域 CSS 下发，各图种的
//! 线宽、字体、底色一致。桑基图、雷达图经实测版式不成熟，暂不支持，check 会给出提示。

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
const STYLE_VERSION: &str = "gongwen-mermaid-v5/merman-0.7";

/// 排版用的字号（px）。引擎的内边距、节点间距是按 14–16px 的字定的，直接拿
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
    // 各图种的标题、坐标刻度统一字号：标题比正文大一档，刻度小一档。
    let title_px = LAYOUT_FONT_PX + 2.0;
    let tick_px = LAYOUT_FONT_PX - 2.0;
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
         text{{fill:{text};}}\
         .cluster-label text,.cluster text{{fill:{stroke};}}\
         .pieTitleText,.titleText,&>text{{font-size:{title_px}px;font-weight:normal;fill:{text};}}\
         .actor,.note,.labelBox,.activation0,.activation1,.activation2\
         {{stroke-width:{node:.2}px;}}\
         .actor-line,.messageLine0,.messageLine1{{stroke-width:{line:.2}px;}}\
         .loopLine{{stroke:{cluster_stroke};stroke-width:{line:.2}px;stroke-dasharray:6 3;}}\
         .task{{stroke-width:{node:.2}px;}}\
         .today{{display:none;}}\
         .grid .tick line{{stroke:{cluster_stroke};stroke-width:{half:.2}px;opacity:1;}}\
         .grid .tick text{{fill:{text};font-size:{tick_px}px;}}\
         .timeline-node .node-bkg{{fill:{decision};stroke:{stroke};stroke-width:{node:.2}px;}}\
         .taskWrapper .timeline-node .node-bkg{{fill:{fill};}}\
         .eventWrapper .timeline-node .node-bkg{{fill:#FFFFFF;}}\
         .timeline-node line{{stroke:none;}}\
         .timeline-node text{{fill:{text};}}\
         .lineWrapper line{{stroke:{line_color};stroke-width:{line:.2}px;}}",
        half = line / 2.0,
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
        // 甘特按版心宽铺开，任务与分段字号同正文；刻度只写月-日，版心宽里排得下。
        .site_config(
            "gantt",
            serde_json::json!({
                "useWidth": text_width,
                "fontSize": LAYOUT_FONT_PX,
                "sectionFontSize": LAYOUT_FONT_PX,
                "barHeight": 24,
                "barGap": 6,
                "topPadding": 40,
                "leftPadding": 80,
                "rightPadding": 20,
                "axisFormat": "%m-%d",
            }),
        )
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

fn render(source: &str, style: Style, theme: DiagramTheme, format: Format) -> Result<Vec<u8>> {
    check(source)?;
    let svg = renderer(style, theme)
        .render_svg_sync(source)
        .map_err(|error| anyhow!("Mermaid 解析或排版失败：{error}"))?
        .context("Mermaid 围栏里没有图形")?;
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
