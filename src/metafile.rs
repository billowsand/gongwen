//! EMF / WMF 矢量图（Windows 图元文件）转 PNG。
//!
//! Word 里的 Visio 图、从旧版 Office 粘过来的图表与剪贴画常以图元文件存放，预览、
//! Word 导出（docx-rs 的图片只收位图）与 Typst 都不认。先用 `emf-core` 把图元文件
//! 转成 SVG（WMF 由它转交 `wmf-core`；EMF+ 双份格式按其中的 GDI 记录画，Office 存的
//! 基本都是这种），再用画 Mermaid 的同一套 resvg 与随包字体光栅化：
//!
//! - 图里记的字体名（宋体、微软雅黑、Arial……）随包字体里没有，按名字归到方正黑体、
//!   公文仿宋或方正书宋（见 [`bundled_family`]），缺的字再按字形回退，不会丢字；
//! - 图元文件的边界常常压着最外侧的笔画，画布四周先留白再画，画完裁掉空白边；
//! - 矢量图放大不失真，统一画到长边约 [`TARGET_LONG_SIDE`] 像素，预览与导出再按版心缩放；
//! - 只有 EMF+ 记录、没有 GDI 备份的图元文件（少见）画出来是空白，按失败处理。
//!
//! `emf-core` 还是 0.1 的开发中版本，解析的又是外来文件，转换包在 `catch_unwind` 里，
//! 一张坏图不能把程序带崩。

use anyhow::{Result, anyhow, bail};
use resvg::{tiny_skia, usvg};

/// 输出 PNG 的长边像素数（目标值）。版心 156 mm 按 300 dpi 约 1843 像素，留些余量。
const TARGET_LONG_SIDE: f32 = 2400.0;
/// 放大倍数上限：图元文件自称很小（图标、符号）时也不必画得比这更细。
const MAX_ZOOM: f32 = 8.0;
/// 画布四周的留白，SVG 单位。
const BLEED: f32 = 4.0;
/// 裁掉空白边后保留的边距，像素。
const MARGIN_PX: u32 = 8;
/// 比这更浅的像素算背景白（三个通道都要达到）。
const WHITE_THRESHOLD: u8 = 250;

const HEI: &str = "FZHei-B01";
const SONG: &str = "FZShuSong-Z01";
const FANG: &str = "GW FangSong";

/// 是不是 EMF / WMF 图元文件（按文件头判断）。
pub(crate) fn is_metafile(bytes: &[u8]) -> bool {
    is_emf(bytes) || is_wmf(bytes)
}

pub(crate) fn is_emf(bytes: &[u8]) -> bool {
    bytes.len() >= 44 && bytes.starts_with(&[1, 0, 0, 0]) && &bytes[40..44] == b" EMF"
}

pub(crate) fn is_wmf(bytes: &[u8]) -> bool {
    // 带 Aldus 可放置头的 WMF，或直接以 META_HEADER 开头（内存型 / 磁盘型）的 WMF。
    bytes.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A])
        || bytes.starts_with(&[0x01, 0x00, 0x09, 0x00])
        || bytes.starts_with(&[0x02, 0x00, 0x09, 0x00])
}

/// 把一份 EMF / WMF 转成 SVG 文本。解析失败只报「无法解析」：错误要拼进状态栏
/// 与正文注释，按原因归并计数，emf-core 的英文细节对用户没有用。
pub(crate) fn to_svg(bytes: &[u8]) -> Result<String> {
    let converted = std::panic::catch_unwind(|| emf_core::converter::convert_to_svg(bytes))
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| anyhow!("无法解析"))?;
    String::from_utf8(converted).map_err(|_| anyhow!("无法解析"))
}

/// 把一份 EMF / WMF 转成 PNG。错误信息是短语，直接拼进「图片未导入」的原因里。
pub(crate) fn to_png(bytes: &[u8]) -> Result<Vec<u8>> {
    let svg = to_svg(bytes)?;
    let options = usvg::Options {
        fontdb: crate::mermaid::fonts(),
        font_family: SONG.into(),
        font_resolver: usvg::FontResolver {
            select_font: Box::new(select_font),
            select_fallback: usvg::FontResolver::default_fallback_selector(),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&svg, &options).map_err(|_| anyhow!("无法光栅化"))?;
    let size = tree.size();
    let long_side = size.width().max(size.height());
    if !(long_side.is_finite() && long_side > 0.0) {
        bail!("尺寸无效");
    }
    let zoom = (TARGET_LONG_SIDE / long_side).min(MAX_ZOOM);
    let width = ((size.width() + 2.0 * BLEED) * zoom).ceil() as u32;
    let height = ((size.height() + 2.0 * BLEED) * zoom).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or_else(|| anyhow!("尺寸无效"))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_translate(BLEED, BLEED).post_scale(zoom, zoom),
        &mut pixmap.as_mut(),
    );
    let cropped = crop_to_content(&pixmap).ok_or_else(|| anyhow!("画面为空"))?;
    cropped.encode_png().map_err(|_| anyhow!("PNG 编码失败"))
}

/// 图元文件里记的字体按名字归到随包字体：黑体与西文无衬线字体用方正黑体，仿宋用
/// 公文仿宋，宋体、小标宋与西文衬线字体用方正书宋。名字逐个看，认不出的（Wingdings、
/// 或 WMF 把 GBK 字节按单字节解出来的乱码名）跳过；一个都认不出就用书宋。
fn bundled_family(families: &[usvg::FontFamily]) -> &'static str {
    const HEI_HINTS: &[&str] = &[
        "黑",
        "hei",
        "yahei",
        "等线",
        "dengxian",
        "arial",
        "calibri",
        "helvetica",
        "segoe",
        "tahoma",
        "verdana",
        "sans",
    ];
    const FANG_HINTS: &[&str] = &["仿宋", "fangsong"];
    const SONG_HINTS: &[&str] = &["宋", "song", "times", "serif", "roman", "明朝"];
    for family in families {
        let name = match family {
            usvg::FontFamily::SansSerif => return HEI,
            usvg::FontFamily::Serif => return SONG,
            usvg::FontFamily::Named(name) => name.to_lowercase(),
            _ => continue,
        };
        // 「仿宋」也含「宋」，先认仿宋。
        if FANG_HINTS.iter().any(|hint| name.contains(hint)) {
            return FANG;
        }
        if HEI_HINTS.iter().any(|hint| name.contains(hint)) {
            return HEI;
        }
        if SONG_HINTS.iter().any(|hint| name.contains(hint)) {
            return SONG;
        }
    }
    SONG
}

fn select_font(
    font: &usvg::Font,
    fontdb: &mut std::sync::Arc<usvg::fontdb::Database>,
) -> Option<usvg::fontdb::ID> {
    let family = bundled_family(font.families());
    fontdb.query(&usvg::fontdb::Query {
        families: &[
            usvg::fontdb::Family::Name(family),
            usvg::fontdb::Family::Serif,
        ],
        weight: usvg::fontdb::Weight(font.weight()),
        ..usvg::fontdb::Query::default()
    })
}

/// 裁掉四周的空白，留 [`MARGIN_PX`] 像素边距；整张都是白的返回 `None`。
fn crop_to_content(pixmap: &tiny_skia::Pixmap) -> Option<tiny_skia::Pixmap> {
    let width = pixmap.width();
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
    for (index, pixel) in pixmap.pixels().iter().enumerate() {
        let inked = pixel.red() < WHITE_THRESHOLD
            || pixel.green() < WHITE_THRESHOLD
            || pixel.blue() < WHITE_THRESHOLD;
        if inked {
            let x = index as u32 % width;
            let y = index as u32 / width;
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
        }
    }
    if left > right {
        return None;
    }
    let left = left.saturating_sub(MARGIN_PX);
    let top = top.saturating_sub(MARGIN_PX);
    let right = (right + MARGIN_PX).min(width - 1);
    let bottom = (bottom + MARGIN_PX).min(pixmap.height() - 1);
    let rect = tiny_skia::IntRect::from_ltrb(
        left as i32,
        top as i32,
        right as i32 + 1,
        bottom as i32 + 1,
    )?;
    pixmap.clone_rect(rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/metafile")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()))
    }

    /// PNG 的宽高与非白像素占比，粗看画面有没有东西。
    fn inspect(png: &[u8]) -> (u32, u32, f64) {
        let image = image::load_from_memory(png).expect("PNG 能解码").to_rgb8();
        let inked = image
            .pixels()
            .filter(|pixel| pixel.0.iter().any(|channel| *channel < WHITE_THRESHOLD))
            .count();
        let total = (image.width() * image.height()) as f64;
        (image.width(), image.height(), inked as f64 / total)
    }

    /// 样本是 GDI+ 录的流程图：画布 460×360，内容只占 x 120–400、y 10–330（约
    /// 280×320，竖长）。EMF+ 双份格式按 GDI 记录画，WMF 走 wmf-core（GDI+ 转出来的
    /// 画布还大一圈）；两者裁掉空白后都该只剩内容，宽高比一致。
    #[test]
    fn emf_and_wmf_render_the_same_trimmed_drawing() {
        let emf = fixture("flow-dual.emf");
        let wmf = fixture("flow.wmf");
        assert!(is_emf(&emf) && !is_wmf(&emf));
        assert!(is_wmf(&wmf) && !is_emf(&wmf));
        assert!(
            to_svg(&emf).unwrap().contains("防汛应急响应流程"),
            "文字要转进 SVG"
        );

        let (emf_width, emf_height, emf_inked) = inspect(&to_png(&emf).unwrap());
        let (wmf_width, wmf_height, wmf_inked) = inspect(&to_png(&wmf).unwrap());
        let emf_ratio = emf_width as f64 / emf_height as f64;
        let wmf_ratio = wmf_width as f64 / wmf_height as f64;
        // 随包字体不入库，CI 上可能没有中文字体：标题画不出来时内容约 250×270，
        // 画得出来约 280×320；原画布 460×360 的宽高比是 1.28，没裁掉空白就超出这个范围。
        assert!(
            (0.8..1.0).contains(&emf_ratio),
            "空白边要裁掉：{emf_width}×{emf_height}"
        );
        assert!(
            (emf_ratio - wmf_ratio).abs() < 0.05,
            "EMF {emf_width}×{emf_height} 与 WMF {wmf_width}×{wmf_height} 应是同一幅图"
        );
        assert!(
            emf_width.max(emf_height) >= 1500,
            "矢量图要放大：{emf_width}×{emf_height}"
        );
        assert!(emf_inked > 0.05 && wmf_inked > 0.05, "画面要有内容");
    }

    /// 只有 EMF+ 记录、没有 GDI 备份：画不出东西，报「画面为空」而不是交一张白图。
    #[test]
    fn emf_plus_only_is_reported_blank() {
        let error = to_png(&fixture("flow-plus-only.emf")).unwrap_err();
        assert_eq!(error.to_string(), "画面为空");
    }

    #[test]
    fn garbage_is_rejected_without_panicking() {
        let mut bytes = vec![1, 0, 0, 0];
        bytes.resize(40, 0xFF);
        bytes.extend_from_slice(b" EMF");
        bytes.resize(200, 0xAB);
        assert!(is_emf(&bytes));
        assert!(to_png(&bytes).is_err());
    }

    #[test]
    fn font_names_map_to_bundled_families() {
        let named = |names: &[&str]| {
            names
                .iter()
                .map(|name| usvg::FontFamily::Named((*name).to_string()))
                .collect::<Vec<_>>()
        };
        assert_eq!(bundled_family(&named(&["微软雅黑"])), HEI);
        assert_eq!(bundled_family(&named(&["Arial"])), HEI);
        assert_eq!(bundled_family(&named(&["仿宋_GB2312"])), FANG);
        assert_eq!(bundled_family(&named(&["宋体"])), SONG);
        assert_eq!(bundled_family(&named(&["Times New Roman"])), SONG);
        // WMF 的乱码名认不出，跳过去认后面的正名。
        assert_eq!(bundled_family(&named(&["Î¢ÈíÑÅºÚ", "微软雅黑"])), HEI);
        assert_eq!(bundled_family(&named(&["Wingdings"])), SONG);
    }

    /// 把 `GONGWEN_METAFILE_SAMPLES` 目录里的 .emf / .wmf 逐个转成 SVG 与 PNG，
    /// 输出到 target/metafile-samples/，供目视核对。
    #[test]
    #[ignore = "需用 GONGWEN_METAFILE_SAMPLES 指定本机 EMF / WMF 样本目录"]
    fn dump_metafile_samples() {
        let dir = std::env::var("GONGWEN_METAFILE_SAMPLES").expect("GONGWEN_METAFILE_SAMPLES");
        let out =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/metafile-samples");
        std::fs::create_dir_all(&out).unwrap();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            if !is_metafile(&bytes) {
                continue;
            }
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            match to_svg(&bytes) {
                Ok(svg) => std::fs::write(out.join(format!("{stem}.svg")), svg).unwrap(),
                Err(error) => eprintln!("{stem}: SVG 失败 {error:#}"),
            }
            match to_png(&bytes) {
                Ok(png) => std::fs::write(out.join(format!("{stem}.png")), png).unwrap(),
                Err(error) => eprintln!("{stem}: PNG 失败 {error:#}"),
            }
        }
    }
}
